// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use back::archive::{Archive, METADATA_FILENAME};
use back::rpath;
use back::svh::Svh;
use driver::driver::{CrateTranslation, OutputFilenames};
use driver::config::NoDebugInfo;
use driver::session::Session;
use driver::config;
use lib::llvm::llvm;
use lib::llvm::ModuleRef;
use lib;
use metadata::common::LinkMeta;
use metadata::{encoder, cstore, filesearch, csearch, loader};
use middle::trans::context::CrateContext;
use middle::trans::common::gensym_name;
use middle::ty;
use util::common::time;
use util::ppaux;
use util::sha2::{Digest, Sha256};

use std::c_str::{ToCStr, CString};
use std::char;
use std::io::{fs, TempDir, Command};
use std::io;
use std::ptr;
use std::str;
use std::string::String;
use std::collections::HashSet;
use flate;
use serialize::hex::ToHex;
use syntax::abi;
use syntax::ast;
use syntax::ast_map::{PathElem, PathElems, PathName};
use syntax::ast_map;
use syntax::attr;
use syntax::attr::AttrMetaMethods;
use syntax::crateid::CrateId;
use syntax::parse::token;

#[deriving(Clone, PartialEq, PartialOrd, Ord, Eq)]
pub enum OutputType {
    OutputTypeBitcode,
    OutputTypeAssembly,
    OutputTypeLlvmAssembly,
    OutputTypeObject,
    OutputTypeExe,
}

pub fn llvm_err(sess: &Session, msg: String) -> ! {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            sess.fatal(msg.as_slice());
        } else {
            let err = CString::new(cstr, true);
            let err = str::from_utf8_lossy(err.as_bytes());
            sess.fatal(format!("{}: {}",
                               msg.as_slice(),
                               err.as_slice()).as_slice());
        }
    }
}
pub fn llvm_actual_err(sess: &Session, msg: String) {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            sess.err(msg.as_slice());
        } else {
            let err = CString::new(cstr, true);
            let err = str::from_utf8_lossy(err.as_bytes());
            sess.err(format!("{}: {}",
                             msg.as_slice(),
                             err.as_slice()).as_slice());
        }
    }
}
pub fn llvm_warn(sess: &Session, msg: String) {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            sess.warn(msg.as_slice());
        } else {
            let err = CString::new(cstr, true);
            let err = str::from_utf8_lossy(err.as_bytes());
            sess.warn(format!("{}: {}",
                              msg.as_slice(),
                              err.as_slice()).as_slice());
        }
    }
}

pub fn write_output_file(
        sess: &Session,
        target: lib::llvm::TargetMachineRef,
        pm: lib::llvm::PassManagerRef,
        m: ModuleRef,
        output: &Path,
        file_type: lib::llvm::FileType) {
    unsafe {
        output.with_c_str(|output| {
            let result = llvm::LLVMRustWriteOutputFile(
                    target, pm, m, output, file_type);
            if !result {
                llvm_err(sess, "could not write output".to_string());
            }
        })
    }
}

pub mod write {

    use back::lto;
    use back::link::{write_output_file, OutputType};
    use back::link::{OutputTypeAssembly, OutputTypeBitcode};
    use back::link::{OutputTypeExe, OutputTypeLlvmAssembly};
    use back::link::{OutputTypeObject};
    use driver::driver::{CrateTranslation, OutputFilenames};
    use driver::config::NoDebugInfo;
    use driver::session::Session;
    use driver::config;
    use lib::llvm::llvm;
    use lib::llvm::{ModuleRef, TargetMachineRef, PassManagerRef};
    use lib;
    use util::common::time;
    use syntax::abi;

    use std::c_str::ToCStr;
    use std::io::{Command};
    use libc::{c_uint, c_int};
    use std::str;

    // On android, we by default compile for armv7 processors. This enables
    // things like double word CAS instructions (rather than emulating them)
    // which are *far* more efficient. This is obviously undesirable in some
    // cases, so if any sort of target feature is specified we don't append v7
    // to the feature list.
    fn target_feature<'a>(sess: &'a Session) -> &'a str {
        match sess.targ_cfg.os {
            abi::OsAndroid => {
                if "" == sess.opts.cg.target_feature.as_slice() {
                    "+v7"
                } else {
                    sess.opts.cg.target_feature.as_slice()
                }
            }
            _ => sess.opts.cg.target_feature.as_slice()
        }
    }

    // Run optimization passes on llmod. Doesn't run LTO passes.
    // Only writes to disk if -C save-temps is used.
    // Used for running passes post link when targeting PNaCl.
    // If you don't want to immediately dispose of the LLVM target machine,
    // pass true to need_target_machine. It will be returned.
    pub fn run_passes_on_mod(sess: &Session,
                             trans: &CrateTranslation,
                             llmod: ModuleRef,
                             output: &OutputFilenames,
                             need_target_machine: bool,
                             disable_verify: bool) -> Option<TargetMachineRef> {
        debug!("running passes");
        unsafe {
            configure_llvm(sess);

            if sess.opts.cg.save_temps {
                output.with_extension("no-opt.bc").with_c_str(|buf| {
                    llvm::LLVMWriteBitcodeToFile(llmod, buf);
                })
            }

            let opt_level = match sess.opts.optimize {
              config::No => lib::llvm::CodeGenLevelNone,
              config::Less => lib::llvm::CodeGenLevelLess,
              config::Default => lib::llvm::CodeGenLevelDefault,
              config::Aggressive => lib::llvm::CodeGenLevelAggressive,
            };
            let use_softfp = sess.opts.cg.soft_float;

            // FIXME: #11906: Omitting frame pointers breaks retrieving the value of a parameter.
            // FIXME: #11954: mac64 unwinding may not work with fp elim
            let no_fp_elim = (sess.opts.debuginfo != NoDebugInfo) ||
                             (sess.targ_cfg.os == abi::OsMacos &&
                              sess.targ_cfg.arch == abi::X86_64);

            // OSX has -dead_strip, which doesn't rely on ffunction_sections
            // FIXME(#13846) this should be enabled for windows
            let ffunction_sections = sess.targ_cfg.os != abi::OsMacos &&
                                     sess.targ_cfg.os != abi::OsWin32;
            let fdata_sections = ffunction_sections;

            let reloc_model = match sess.opts.cg.relocation_model.as_slice() {
                "pic" => lib::llvm::RelocPIC,
                "static" => lib::llvm::RelocStatic,
                "default" => lib::llvm::RelocDefault,
                "dynamic-no-pic" => lib::llvm::RelocDynamicNoPic,
                _ => {
                    sess.fatal(format!("{} is not a valid relocation mode",
                                     sess.opts
                                         .cg
                                         .relocation_model).as_slice());
                }
            };

            let tm = sess.targ_cfg
                         .target_strs
                         .target_triple
                         .as_slice()
                         .with_c_str(|t| {
                sess.opts.cg.target_cpu.as_slice().with_c_str(|cpu| {
                    target_feature(sess).with_c_str(|features| {
                        llvm::LLVMRustCreateTargetMachine(
                            t, cpu, features,
                            lib::llvm::CodeModelDefault,
                            reloc_model,
                            opt_level,
                            true /* EnableSegstk */,
                            use_softfp,
                            no_fp_elim,
                            ffunction_sections,
                            fdata_sections,
                        )
                    })
                })
            });

            // Create the two optimizing pass managers. These mirror what clang
            // does, and are by populated by LLVM's default PassManagerBuilder.
            // Each manager has a different set of passes, but they also share
            // some common passes.
            let fpm = llvm::LLVMCreateFunctionPassManagerForModule(llmod);
            let mpm = llvm::LLVMCreatePassManager();

            let addpass = |pass: &str| {
                pass.as_slice().with_c_str(|s| llvm::LLVMRustAddPass(fpm, s))
            };
            let addpass_mpm = |pass: &str| {
                pass.with_c_str(|s| llvm::LLVMRustAddPass(mpm, s))
            };
            // If we're verifying or linting, add them to the function pass
            // manager.
            if !sess.no_verify() && !disable_verify { assert!(addpass("verify")); }

            if sess.targeting_pnacl() && !sess.opts.cg.no_prepopulate_passes {
                // I choose to add these by string to retain what little compatibility
                // we have left with upstream LLVM
                assert!(addpass_mpm("pnacl-sjlj-eh"));
                assert!(addpass_mpm("expand-indirectbr"));
                assert!(addpass_mpm("lower-expect"));
                assert!(addpass_mpm("rewrite-llvm-intrinsic-calls"));
                assert!(addpass_mpm("expand-arith-with-overflow"));
                assert!(addpass_mpm("replace-vectors-with-arrays"));
                assert!(addpass_mpm("promote-simple-structs"));
                assert!(addpass_mpm("promote-returned-structures"));
                assert!(addpass_mpm("promote-structure-arguments"));
                assert!(addpass_mpm("expand-struct-regs"));
                assert!(addpass_mpm("expand-varargs"));
                assert!(addpass_mpm("nacl-expand-ctors"));
                assert!(addpass_mpm("nacl-expand-tls-constant-expr"));
            }

            if !sess.opts.cg.no_prepopulate_passes {
                llvm::LLVMRustAddAnalysisPasses(tm, fpm, llmod);
                llvm::LLVMRustAddAnalysisPasses(tm, mpm, llmod);
                populate_llvm_passes(fpm, mpm, llmod, opt_level,
                                     trans.no_builtins);
            }

            for pass in sess.opts.cg.passes.iter() {
                pass.as_slice().with_c_str(|s| {
                    if !llvm::LLVMRustAddPass(mpm, s) {
                        sess.warn(format!("unknown pass {}, ignoring",
                                          *pass).as_slice());
                    }
                })
            }

            if sess.targeting_pnacl() && !sess.opts.cg.no_prepopulate_passes {
                // I choose to add these by string to retain what little compatibility
                // we have left with upstream LLVM
                assert!(addpass_mpm("rewrite-pnacl-library-calls"));
                assert!(addpass_mpm("expand-byval"));
                assert!(addpass_mpm("expand-small-arguments"));
                assert!(addpass_mpm("nacl-promote-i1-ops"));
                assert!(addpass_mpm("canonicalize-mem-intrinsics"));
                assert!(addpass_mpm("flatten-globals"));
                assert!(addpass_mpm("expand-constant-expr"));
                assert!(addpass_mpm("nacl-promote-ints"));
                assert!(addpass_mpm("expand-getelementptr"));
                assert!(addpass_mpm("nacl-rewrite-atomics"));
                assert!(addpass_mpm("remove-asm-memory"));
                assert!(addpass_mpm("replace-ptrs-with-ints"));
                assert!(addpass_mpm("replace-aggregates-with-ints"));
                assert!(addpass_mpm("die"));
                assert!(addpass_mpm("dce"));

                if !sess.no_verify() && !disable_verify { assert!(addpass_mpm("verify")); }
            }

            // Finally, run the actual optimization passes
            time(sess.time_passes(), "llvm function passes", (), |()|
                 llvm::LLVMRustRunFunctionPassManager(fpm, llmod));
            time(sess.time_passes(), "llvm module passes", (), |()|
                 llvm::LLVMRunPassManager(mpm, llmod));

            // Deallocate managers that we're now done with
            llvm::LLVMDisposePassManager(fpm);
            llvm::LLVMDisposePassManager(mpm);

            if sess.time_llvm_passes() { llvm::LLVMRustPrintPassTimings(); }

            debug!("done running passes");

            if !need_target_machine {
                llvm::LLVMRustDisposeTargetMachine(tm);
                None
            } else {
                Some(tm)
            }
        }
    }

    pub fn run_passes(sess: &Session,
                      trans: &CrateTranslation,
                      output_types: &[OutputType],
                      output: &OutputFilenames) {
        let llmod = trans.module;
        let llcx = trans.context;

        let tm = run_passes_on_mod(sess, trans, llmod, output, true, false).unwrap();

        unsafe {
            // Emit the bytecode if we're either saving our temporaries or
            // emitting an rlib. Whenever an rlib is created, the bytecode is
            // inserted into the archive in order to allow LTO against it.
            if sess.opts.cg.save_temps ||
               (sess.crate_types.borrow().contains(&config::CrateTypeRlib) &&
                sess.opts.output_types.contains(&OutputTypeExe)) {
                output.temp_path(OutputTypeBitcode).with_c_str(|buf| {
                    llvm::LLVMWriteBitcodeToFile(llmod, buf);
                })
            }

            if sess.lto() {
                time(sess.time_passes(), "all lto passes", (), |()|
                     lto::run(sess, llmod, tm, trans.reachable.as_slice()));

                if sess.opts.cg.save_temps {
                    output.with_extension("lto.bc").with_c_str(|buf| {
                        llvm::LLVMWriteBitcodeToFile(llmod, buf);
                    })
                }
            }

            // A codegen-specific pass manager is used to generate object
            // files for an LLVM module.
            //
            // Apparently each of these pass managers is a one-shot kind of
            // thing, so we create a new one for each type of output. The
            // pass manager passed to the closure should be ensured to not
            // escape the closure itself, and the manager should only be
            // used once.
            fn with_codegen(tm: TargetMachineRef, llmod: ModuleRef,
                            no_builtins: bool, f: |PassManagerRef|) {
                unsafe {
                    let cpm = llvm::LLVMCreatePassManager();
                    llvm::LLVMRustAddAnalysisPasses(tm, cpm, llmod);
                    llvm::LLVMRustAddLibraryInfo(cpm, llmod, no_builtins);
                    f(cpm);
                    llvm::LLVMDisposePassManager(cpm);
                }
            }

            let mut object_file = None;
            let mut needs_metadata = false;
            for output_type in output_types.iter() {
                let path = output.path(*output_type);
                match *output_type {
                    OutputTypeBitcode => {
                        path.with_c_str(|buf| {
                            llvm::LLVMWriteBitcodeToFile(llmod, buf);
                        })
                    }
                    OutputTypeLlvmAssembly if sess.targeting_pnacl() => {
                        output.temp_path(*output_type).with_c_str(|output| {
                            with_codegen(tm, llmod, trans.no_builtins, |cpm| {
                                llvm::LLVMRustPrintModule(cpm, llmod, output);
                            })
                        });
                        output.with_extension("metadata.ll").with_c_str(|output| {
                            with_codegen(tm, trans.metadata_module, trans.no_builtins, |cpm| {
                                llvm::LLVMRustPrintModule(cpm,
                                                          trans.metadata_module,
                                                          output);
                            })
                        })
                    }
                    OutputTypeLlvmAssembly => {
                        path.with_c_str(|output| {
                            with_codegen(tm, llmod, trans.no_builtins, |cpm| {
                                llvm::LLVMRustPrintModule(cpm, llmod, output);
                            })
                        })
                    }
                    OutputTypeAssembly => {
                        // If we're not using the LLVM assembler, this function
                        // could be invoked specially with output_type_assembly,
                        // so in this case we still want the metadata object
                        // file.
                        let ty = OutputTypeAssembly;
                        let path = if sess.opts.output_types.contains(&ty) {
                           path
                        } else {
                            needs_metadata = true;
                            output.temp_path(OutputTypeAssembly)
                        };
                        with_codegen(tm, llmod, trans.no_builtins, |cpm| {
                            write_output_file(sess, tm, cpm, llmod, &path,
                                            lib::llvm::AssemblyFile);
                        });
                    }
                    OutputTypeObject => {
                        object_file = Some(path);
                    }
                    OutputTypeExe => {
                        object_file = Some(output.temp_path(OutputTypeObject));
                        needs_metadata = true;
                    }
                }
            }

            time(sess.time_passes(), "codegen passes", (), |()| {
                match object_file {
                    Some(ref path) => {
                        with_codegen(tm, llmod, trans.no_builtins, |cpm| {
                            write_output_file(sess, tm, cpm, llmod, path,
                                            lib::llvm::ObjectFile);
                        });
                    }
                    None => {}
                }
                if needs_metadata {
                    with_codegen(tm, trans.metadata_module,
                                 trans.no_builtins, |cpm| {
                        let out = output.temp_path(OutputTypeObject)
                                        .with_extension("metadata.o");
                        write_output_file(sess, tm, cpm,
                                        trans.metadata_module, &out,
                                        lib::llvm::ObjectFile);
                    })
                }
            });

            llvm::LLVMRustDisposeTargetMachine(tm);
            llvm::LLVMDisposeModule(trans.metadata_module);
            llvm::LLVMDisposeModule(llmod);
            llvm::LLVMContextDispose(llcx);
            if sess.time_llvm_passes() { llvm::LLVMRustPrintPassTimings(); }
        }
    }

    pub fn run_assembler(sess: &Session, outputs: &OutputFilenames) {
        let pname = super::get_cc_prog(sess);
        let mut cmd = Command::new(pname.as_slice());

        cmd.arg("-c").arg("-o").arg(outputs.path(OutputTypeObject))
                               .arg(outputs.temp_path(OutputTypeAssembly));
        debug!("{}", &cmd);

        match cmd.output() {
            Ok(prog) => {
                if !prog.status.success() {
                    sess.err(format!("linking with `{}` failed: {}",
                                     pname,
                                     prog.status).as_slice());
                    sess.note(format!("{}", &cmd).as_slice());
                    let mut note = prog.error.clone();
                    note.push_all(prog.output.as_slice());
                    sess.note(str::from_utf8(note.as_slice()).unwrap()
                                                             .as_slice());
                    sess.abort_if_errors();
                }
            },
            Err(e) => {
                sess.err(format!("could not exec the linker `{}`: {}",
                                 pname,
                                 e).as_slice());
                sess.abort_if_errors();
            }
        }
    }

    unsafe fn configure_llvm(sess: &Session) {
        use sync::one::{Once, ONCE_INIT};
        static mut INIT: Once = ONCE_INIT;

        // Copy what clang does by turning on loop vectorization at O2 and
        // slp vectorization at O3
        let vectorize_loop = !sess.opts.cg.no_vectorize_loops &&
                             (sess.opts.optimize == config::Default ||
                              sess.opts.optimize == config::Aggressive) &&
                             !sess.targeting_pnacl();
        let vectorize_slp = !sess.opts.cg.no_vectorize_slp &&
                             sess.opts.optimize == config::Aggressive &&
                            !sess.targeting_pnacl();

        let mut llvm_c_strs = Vec::new();
        let mut llvm_args = Vec::new();
        {
            let add = |arg: &str| {
                let s = arg.to_c_str();
                llvm_args.push(s.with_ref(|p| p));
                llvm_c_strs.push(s);
            };
            add("rustc"); // fake program name
            if vectorize_loop { add("-vectorize-loops"); }
            if vectorize_slp  { add("-vectorize-slp");   }
            if sess.time_llvm_passes() { add("-time-passes"); }
            if sess.print_llvm_passes() { add("-debug-pass=Structure"); }

            if sess.targeting_pnacl() {
                add("-pnaclabi-allow-debug-metadata");
            }

            for arg in sess.opts.cg.llvm_args.iter() {
                add((*arg).as_slice());
            }
        }

        INIT.doit(|| {
            llvm::LLVMInitializePasses();

            // Only initialize the platforms supported by Rust here, because
            // using --llvm-root will have multiple platforms that rustllvm
            // doesn't actually link to and it's pointless to put target info
            // into the registry that Rust cannot generate machine code for.
            llvm::LLVMInitializeX86TargetInfo();
            llvm::LLVMInitializeX86Target();
            llvm::LLVMInitializeX86TargetMC();
            llvm::LLVMInitializeX86AsmPrinter();
            llvm::LLVMInitializeX86AsmParser();

            llvm::LLVMInitializeARMTargetInfo();
            llvm::LLVMInitializeARMTarget();
            llvm::LLVMInitializeARMTargetMC();
            llvm::LLVMInitializeARMAsmPrinter();
            llvm::LLVMInitializeARMAsmParser();

            llvm::LLVMInitializeMipsTargetInfo();
            llvm::LLVMInitializeMipsTarget();
            llvm::LLVMInitializeMipsTargetMC();
            llvm::LLVMInitializeMipsAsmPrinter();
            llvm::LLVMInitializeMipsAsmParser();

            llvm::LLVMRustSetLLVMOptions(llvm_args.len() as c_int,
                                         llvm_args.as_ptr());
        });
    }

    unsafe fn populate_llvm_passes(fpm: lib::llvm::PassManagerRef,
                                   mpm: lib::llvm::PassManagerRef,
                                   llmod: ModuleRef,
                                   opt: lib::llvm::CodeGenOptLevel,
                                   no_builtins: bool) {
        // Create the PassManagerBuilder for LLVM. We configure it with
        // reasonable defaults and prepare it to actually populate the pass
        // manager.
        let builder = llvm::LLVMPassManagerBuilderCreate();
        match opt {
            lib::llvm::CodeGenLevelNone => {
                // Don't add lifetime intrinsics at O0
                llvm::LLVMRustAddAlwaysInlinePass(builder, false);
            }
            lib::llvm::CodeGenLevelLess => {
                llvm::LLVMRustAddAlwaysInlinePass(builder, true);
            }
            // numeric values copied from clang
            lib::llvm::CodeGenLevelDefault => {
                llvm::LLVMPassManagerBuilderUseInlinerWithThreshold(builder,
                                                                    225);
            }
            lib::llvm::CodeGenLevelAggressive => {
                llvm::LLVMPassManagerBuilderUseInlinerWithThreshold(builder,
                                                                    275);
            }
        }
        llvm::LLVMPassManagerBuilderSetOptLevel(builder, opt as c_uint);
        llvm::LLVMRustAddBuilderLibraryInfo(builder, llmod, no_builtins);

        // Use the builder to populate the function/module pass managers.
        llvm::LLVMPassManagerBuilderPopulateFunctionPassManager(builder, fpm);
        llvm::LLVMPassManagerBuilderPopulateModulePassManager(builder, mpm);
        llvm::LLVMPassManagerBuilderDispose(builder);
    }
}


/*
 * Name mangling and its relationship to metadata. This is complex. Read
 * carefully.
 *
 * The semantic model of Rust linkage is, broadly, that "there's no global
 * namespace" between crates. Our aim is to preserve the illusion of this
 * model despite the fact that it's not *quite* possible to implement on
 * modern linkers. We initially didn't use system linkers at all, but have
 * been convinced of their utility.
 *
 * There are a few issues to handle:
 *
 *  - Linkers operate on a flat namespace, so we have to flatten names.
 *    We do this using the C++ namespace-mangling technique. Foo::bar
 *    symbols and such.
 *
 *  - Symbols with the same name but different types need to get different
 *    linkage-names. We do this by hashing a string-encoding of the type into
 *    a fixed-size (currently 16-byte hex) cryptographic hash function (CHF:
 *    we use SHA256) to "prevent collisions". This is not airtight but 16 hex
 *    digits on uniform probability means you're going to need 2**32 same-name
 *    symbols in the same process before you're even hitting birthday-paradox
 *    collision probability.
 *
 *  - Symbols in different crates but with same names "within" the crate need
 *    to get different linkage-names.
 *
 *  - The hash shown in the filename needs to be predictable and stable for
 *    build tooling integration. It also needs to be using a hash function
 *    which is easy to use from Python, make, etc.
 *
 * So here is what we do:
 *
 *  - Consider the package id; every crate has one (specified with crate_id
 *    attribute).  If a package id isn't provided explicitly, we infer a
 *    versionless one from the output name. The version will end up being 0.0
 *    in this case. CNAME and CVERS are taken from this package id. For
 *    example, github.com/mozilla/CNAME#CVERS.
 *
 *  - Define CMH as SHA256(crateid).
 *
 *  - Define CMH8 as the first 8 characters of CMH.
 *
 *  - Compile our crate to lib CNAME-CMH8-CVERS.so
 *
 *  - Define STH(sym) as SHA256(CMH, type_str(sym))
 *
 *  - Suffix a mangled sym with ::STH@CVERS, so that it is unique in the
 *    name, non-name metadata, and type sense, and versioned in the way
 *    system linkers understand.
 */

// FIXME (#9639): This needs to handle non-utf8 `out_filestem` values
pub fn find_crate_id(attrs: &[ast::Attribute], out_filestem: &str) -> CrateId {
    match attr::find_crateid(attrs) {
        None => from_str(out_filestem).unwrap_or_else(|| {
            let mut s = out_filestem.chars().filter(|c| c.is_XID_continue());
            from_str(s.collect::<String>().as_slice())
                .or(from_str("rust-out")).unwrap()
        }),
        Some(s) => s,
    }
}

pub fn crate_id_hash(crate_id: &CrateId) -> String {
    // This calculates CMH as defined above. Note that we don't use the path of
    // the crate id in the hash because lookups are only done by (name/vers),
    // not by path.
    let mut s = Sha256::new();
    s.input_str(crate_id.short_name_with_version().as_slice());
    truncated_hash_result(&mut s).as_slice().slice_to(8).to_string()
}

// FIXME (#9639): This needs to handle non-utf8 `out_filestem` values
pub fn build_link_meta(krate: &ast::Crate,
                       out_filestem: &str) -> LinkMeta {
    let r = LinkMeta {
        crateid: find_crate_id(krate.attrs.as_slice(), out_filestem),
        crate_hash: Svh::calculate(krate),
    };
    info!("{}", r);
    return r;
}

fn truncated_hash_result(symbol_hasher: &mut Sha256) -> String {
    let output = symbol_hasher.result_bytes();
    // 64 bits should be enough to avoid collisions.
    output.slice_to(8).to_hex().to_string()
}


// This calculates STH for a symbol, as defined above
fn symbol_hash(tcx: &ty::ctxt,
               symbol_hasher: &mut Sha256,
               t: ty::t,
               link_meta: &LinkMeta)
               -> String {
    // NB: do *not* use abbrevs here as we want the symbol names
    // to be independent of one another in the crate.

    symbol_hasher.reset();
    symbol_hasher.input_str(link_meta.crateid.name.as_slice());
    symbol_hasher.input_str("-");
    symbol_hasher.input_str(link_meta.crate_hash.as_str());
    symbol_hasher.input_str("-");
    symbol_hasher.input_str(encoder::encoded_ty(tcx, t).as_slice());
    // Prefix with 'h' so that it never blends into adjacent digits
    let mut hash = String::from_str("h");
    hash.push_str(truncated_hash_result(symbol_hasher).as_slice());
    hash
}

fn get_symbol_hash(ccx: &CrateContext, t: ty::t) -> String {
    match ccx.type_hashcodes.borrow().find(&t) {
        Some(h) => return h.to_string(),
        None => {}
    }

    let mut symbol_hasher = ccx.symbol_hasher.borrow_mut();
    let hash = symbol_hash(ccx.tcx(), &mut *symbol_hasher, t, &ccx.link_meta);
    ccx.type_hashcodes.borrow_mut().insert(t, hash.clone());
    hash
}


// Name sanitation. LLVM will happily accept identifiers with weird names, but
// gas doesn't!
// gas accepts the following characters in symbols: a-z, A-Z, 0-9, ., _, $
pub fn sanitize(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            // Escape these with $ sequences
            '@' => result.push_str("$SP$"),
            '~' => result.push_str("$UP$"),
            '*' => result.push_str("$RP$"),
            '&' => result.push_str("$BP$"),
            '<' => result.push_str("$LT$"),
            '>' => result.push_str("$GT$"),
            '(' => result.push_str("$LP$"),
            ')' => result.push_str("$RP$"),
            ',' => result.push_str("$C$"),

            // '.' doesn't occur in types and functions, so reuse it
            // for ':' and '-'
            '-' | ':' => result.push_char('.'),

            // These are legal symbols
            'a' .. 'z'
            | 'A' .. 'Z'
            | '0' .. '9'
            | '_' | '.' | '$' => result.push_char(c),

            _ => {
                let mut tstr = String::new();
                char::escape_unicode(c, |c| tstr.push_char(c));
                result.push_char('$');
                result.push_str(tstr.as_slice().slice_from(1));
            }
        }
    }

    // Underscore-qualify anything that didn't start as an ident.
    if result.len() > 0u &&
        result.as_slice()[0] != '_' as u8 &&
        ! char::is_XID_start(result.as_slice()[0] as char) {
        return format!("_{}", result.as_slice()).to_string();
    }

    return result;
}

pub fn mangle<PI: Iterator<PathElem>>(mut path: PI,
                                      hash: Option<&str>,
                                      vers: Option<&str>) -> String {
    // Follow C++ namespace-mangling style, see
    // http://en.wikipedia.org/wiki/Name_mangling for more info.
    //
    // It turns out that on OSX you can actually have arbitrary symbols in
    // function names (at least when given to LLVM), but this is not possible
    // when using unix's linker. Perhaps one day when we just use a linker from LLVM
    // we won't need to do this name mangling. The problem with name mangling is
    // that it seriously limits the available characters. For example we can't
    // have things like &T or ~[T] in symbol names when one would theoretically
    // want them for things like impls of traits on that type.
    //
    // To be able to work on all platforms and get *some* reasonable output, we
    // use C++ name-mangling.

    let mut n = String::from_str("_ZN"); // _Z == Begin name-sequence, N == nested

    fn push(n: &mut String, s: &str) {
        let sani = sanitize(s);
        n.push_str(format!("{}{}", sani.len(), sani).as_slice());
    }

    // First, connect each component with <len, name> pairs.
    for e in path {
        push(&mut n, token::get_name(e.name()).get().as_slice())
    }

    match hash {
        Some(s) => push(&mut n, s),
        None => {}
    }
    match vers {
        Some(s) => push(&mut n, s),
        None => {}
    }

    n.push_char('E'); // End name-sequence.
    n
}

pub fn exported_name(path: PathElems, hash: &str, vers: &str) -> String {
    // The version will get mangled to have a leading '_', but it makes more
    // sense to lead with a 'v' b/c this is a version...
    let vers = if vers.len() > 0 && !char::is_XID_start(vers.char_at(0)) {
        format!("v{}", vers)
    } else {
        vers.to_string()
    };

    mangle(path, Some(hash), Some(vers.as_slice()))
}

pub fn mangle_exported_name(ccx: &CrateContext, path: PathElems,
                            t: ty::t, id: ast::NodeId) -> String {
    let mut hash = get_symbol_hash(ccx, t);

    // Paths can be completely identical for different nodes,
    // e.g. `fn foo() { { fn a() {} } { fn a() {} } }`, so we
    // generate unique characters from the node id. For now
    // hopefully 3 characters is enough to avoid collisions.
    static EXTRA_CHARS: &'static str =
        "abcdefghijklmnopqrstuvwxyz\
         ABCDEFGHIJKLMNOPQRSTUVWXYZ\
         0123456789";
    let id = id as uint;
    let extra1 = id % EXTRA_CHARS.len();
    let id = id / EXTRA_CHARS.len();
    let extra2 = id % EXTRA_CHARS.len();
    let id = id / EXTRA_CHARS.len();
    let extra3 = id % EXTRA_CHARS.len();
    hash.push_char(EXTRA_CHARS[extra1] as char);
    hash.push_char(EXTRA_CHARS[extra2] as char);
    hash.push_char(EXTRA_CHARS[extra3] as char);

    exported_name(path,
                  hash.as_slice(),
                  ccx.link_meta.crateid.version_or_default())
}

pub fn mangle_internal_name_by_type_and_seq(ccx: &CrateContext,
                                            t: ty::t,
                                            name: &str) -> String {
    let s = ppaux::ty_to_str(ccx.tcx(), t);
    let path = [PathName(token::intern(s.as_slice())),
                gensym_name(name)];
    let hash = get_symbol_hash(ccx, t);
    mangle(ast_map::Values(path.iter()), Some(hash.as_slice()), None)
}

pub fn mangle_internal_name_by_path_and_seq(path: PathElems, flav: &str) -> String {
    mangle(path.chain(Some(gensym_name(flav)).move_iter()), None, None)
}

pub fn output_lib_filename(id: &CrateId) -> String {
    format!("{}-{}-{}", id.name, crate_id_hash(id), id.version_or_default())
}

pub fn get_cc_prog(sess: &Session) -> String {
    match sess.opts.cg.linker {
        Some(ref linker) => return linker.to_string(),
        None => {}
    }

    // In the future, FreeBSD will use clang as default compiler.
    // It would be flexible to use cc (system's default C compiler)
    // instead of hard-coded gcc.
    // For win32, there is no cc command, so we add a condition to make it use gcc.
    match sess.targ_cfg.os {
        abi::OsWin32 => "gcc",
        _ => "cc",
    }.to_string()
}

pub fn get_ar_prog(sess: &Session) -> String {
    match sess.opts.cg.ar {
        Some(ref ar) => (*ar).clone(),
        None => "ar".to_string()
    }
}

fn remove(sess: &Session, path: &Path) {
    match fs::unlink(path) {
        Ok(..) => {}
        Err(e) => {
            sess.err(format!("failed to remove {}: {}",
                             path.display(),
                             e).as_slice());
        }
    }
}

/// Perform the linkage portion of the compilation phase. This will generate all
/// of the requested outputs for this compilation session.
pub fn link_binary(sess: &Session,
                   trans: &CrateTranslation,
                   outputs: &OutputFilenames,
                   id: &CrateId) -> Vec<Path> {
    let mut out_filenames = Vec::new();
    for &crate_type in sess.crate_types.borrow().iter() {
        if sess.targeting_pnacl() && crate_type == config::CrateTypeDylib {
            sess.warn("skipping dylib output; PNaCl doesn't support it");
        } else {
            let out_file = link_binary_output(sess, trans, crate_type, outputs, id);
            out_filenames.push(out_file);
        }
    }

    // Remove the temporary object file and metadata if we aren't saving temps
    if !sess.opts.cg.save_temps {
        let obj_filename = outputs.temp_path(OutputTypeObject);
        if !sess.opts.output_types.contains(&OutputTypeObject) {
            remove(sess, &obj_filename);
        }
        remove(sess, &obj_filename.with_extension("metadata.o"));
    }

    out_filenames
}

fn is_writeable(p: &Path) -> bool {
    match p.stat() {
        Err(..) => true,
        Ok(m) => m.perm & io::UserWrite == io::UserWrite
    }
}

pub fn filename_for_input(sess: &Session, crate_type: config::CrateType,
                          id: &CrateId, out_filename: &Path) -> Path {
    let libname = output_lib_filename(id);
    match crate_type {
        config::CrateTypeRlib => {
            out_filename.with_filename(format!("lib{}.rlib", libname))
        }
        config::CrateTypeDylib => {
            let (prefix, suffix) = match sess.targ_cfg.os {
                abi::OsWin32 => (loader::WIN32_DLL_PREFIX, loader::WIN32_DLL_SUFFIX),
                abi::OsMacos => (loader::MACOS_DLL_PREFIX, loader::MACOS_DLL_SUFFIX),
                abi::OsLinux => (loader::LINUX_DLL_PREFIX, loader::LINUX_DLL_SUFFIX),
                abi::OsAndroid => (loader::ANDROID_DLL_PREFIX, loader::ANDROID_DLL_SUFFIX),
                abi::OsFreebsd => (loader::FREEBSD_DLL_PREFIX, loader::FREEBSD_DLL_SUFFIX),
                // FIXME: These are merely placeholders.
                abi::OsNaCl => (loader::NACL_DLL_PREFIX, loader::NACL_DLL_SUFFIX),
            };
            out_filename.with_filename(format!("{}{}{}", prefix, libname,
                                               suffix))
        }
        config::CrateTypeStaticlib => {
            out_filename.with_filename(format!("lib{}.a", libname))
        }
        config::CrateTypeExecutable => out_filename.clone(),
    }
}

fn link_binary_output(sess: &Session,
                      trans: &CrateTranslation,
                      crate_type: config::CrateType,
                      outputs: &OutputFilenames,
                      id: &CrateId) -> Path {
    let (obj_filename, out_filename) = check_outputs(sess, crate_type,
                                                     outputs, id);

    match crate_type {
        config::CrateTypeRlib => {
            link_rlib(sess, Some(trans), &obj_filename, &out_filename);
        }
        config::CrateTypeStaticlib => {
            link_staticlib(sess, &obj_filename, &out_filename);
        }
        config::CrateTypeExecutable => {
            link_natively(sess, trans, false, &obj_filename, &out_filename);
        }
        config::CrateTypeDylib => {
            link_natively(sess, trans, true, &obj_filename, &out_filename);
        }
    }

    out_filename
}

pub fn check_outputs(sess: &Session,
                     crate_type: config::CrateType,
                     outputs: &OutputFilenames,
                     id: &CrateId) -> (Path, Path) {
    let obj_filename = outputs.temp_path(OutputTypeObject);
    let out_filename = match outputs.single_output_file {
        Some(ref file) => file.clone(),
        None => {
            let out_filename = outputs.path(OutputTypeExe);
            filename_for_input(sess, crate_type, id, &out_filename)
        }
    };

    // Make sure the output and obj_filename are both writeable.
    // Mac, FreeBSD, and Windows system linkers check this already --
    // however, the Linux linker will happily overwrite a read-only file.
    // We should be consistent.
    let obj_is_writeable = is_writeable(&obj_filename);
    let out_is_writeable = is_writeable(&out_filename);
    if !out_is_writeable {
        sess.fatal(format!("output file {} is not writeable -- check its \
                            permissions.",
                           out_filename.display()).as_slice());
    }
    else if !obj_is_writeable {
        sess.fatal(format!("object file {} is not writeable -- check its \
                            permissions.",
                           obj_filename.display()).as_slice());
    }
    (obj_filename, out_filename)
}

fn pnacl_host_tool(sess: &Session, tool: &str) -> Path {
    use std::os;
    let tool = format!("le32-nacl-{}{}", tool, os::consts::EXE_SUFFIX);
    // Somewhat hacky...
    sess.host_filesearch().get_lib_path().join("../bin").join(tool)
}

fn link_pnacl_rlib(sess: &Session,
                   trans: &CrateTranslation,
                   outputs: &OutputFilenames,
                   id: &CrateId) {
    use libc;
    use back::archive::ArchiveRO;

    // In this case, all archives just have bitcode in them. Instead of
    // running all PNaCl passes when we produce pexe's, preprocess the
    // bitcode files once here.

    write::run_passes_on_mod(sess,
                             trans,
                             trans.module,
                             outputs,
                             false,
                             false);

    let (obj_filename, out_filename) = check_outputs(sess, config::CrateTypeRlib,
                                                     outputs, id);
    obj_filename.with_c_str(|s| unsafe {
        llvm::LLVMWriteBitcodeToFile(trans.module, s);
    });

    let mut a = Archive::new(sess,
                             pnacl_host_tool(sess, "ar"),
                             out_filename.clone(),
                             &obj_filename,
                             Some(sess.gold_plugin_path()));
    sess.remove_temp(&obj_filename, OutputTypeObject);
    let ctxt = trans.context;
    let used = sess.cstore.get_used_libraries().borrow();
    let mut linked = already_linked_libs
        (sess, &sess.cstore.get_used_crates(cstore::RequireStatic));
    let lib_paths = pnacl_lib_paths(sess);
    let tmp = TempDir::new("rlib-bitcode").expect("needs tempdir");
    (*used)
        .iter()
        .filter_map(|&(ref lib, kind)| {
            link_attrs_filter(sess, lib, kind, &mut linked, &lib_paths)
        })
        .fold(0, |_, (l, p): (String, Path)| {
            debug!("processing archive `{}`", p.display());
            let archive = ArchiveRO::open(&p)
                .expect("maybe invalid archive?");
            archive.foreach_child(|name, bc| {
                let name_path = Path::new(name);
                let name_ext_str = name_path.extension_str();
                if name_ext_str == Some("o") || name_ext_str == Some("obj") {
                    let llmod = name.with_c_str(|s| unsafe {
                        llvm::LLVMRustParseBitcode(ctxt,
                                                   s,
                                                   bc.as_ptr() as *libc::c_void,
                                                   bc.len() as libc::size_t)
                    });
                    if llmod == ptr::null() {
                        let msg = format!("failed to parse external bitcode
                                          `{}` in archive `{}`",
                                          name, p.display());
                        llvm_actual_err(sess, msg);
                    } else {
                        let outs = OutputFilenames {
                            out_directory: tmp.path().clone(),
                            out_filestem:  name.to_string(),
                            single_output_file: None,
                        };

                        unsafe {
                            llvm::LLVMRustStripDebugInfo(llmod);
                        }

                        // Some globals in the bitcode from PNaCl have what is
                        // considered invalid linkage in our LLVM (their LLVM is
                        // old). Fortunately, all linkage types get stripped later, so
                        // it's safe to just ignore them all.
                        write::run_passes_on_mod(sess,
                                                 trans,
                                                 llmod,
                                                 &outs,
                                                 false,
                                                 true);

                        let p = tmp.path().join(format!("r-{}-{}", l, name));
                        p.with_c_str(|buf| unsafe {
                            llvm::LLVMWriteBitcodeToFile(llmod, buf);
                            llvm::LLVMDisposeModule(llmod);
                        });
                        a.add_file(&p, false);
                    }
                }
            });
            0u32
        });

    // Instead of putting the metadata in an object file section, rlibs
    // contain the metadata in a separate file. We use a temp directory
    // here so concurrent builds in the same directory don't try to use
    // the same filename for metadata (stomping over one another)
    debug!("adding metadata to archive");
    let metadata = tmp.path().join(METADATA_FILENAME);
    match fs::File::create(&metadata).write(trans.metadata
                                            .as_slice()) {
        Ok(..) => {}
        Err(e) => {
            sess.fatal(format!("failed to write {}: {}",
                               metadata.display(), e).as_slice());
        }
    }
    a.add_file(&metadata, false);
    remove(sess, &metadata);

    a.update_symbols();

    sess.create_temp(OutputTypeBitcode, || {
        let path = outputs.path(OutputTypeBitcode);
        sess.check_writeable_output(&path, "bitcode");
        path.with_c_str(|buf| unsafe {
            llvm::LLVMWriteBitcodeToFile(trans.module, buf);
        })
    });

    // Note no update of symbol table. pnacl-ar can't read our
    // bitcode even if the resultant table was used.

    if sess.opts.cg.save_temps {
        tmp.unwrap();
    }

    sess.abort_if_errors();
}

pub fn link_outputs_for_pnacl(sess: &Session,
                              trans: &mut CrateTranslation,
                              outputs: &OutputFilenames,
                              id: CrateId) {
    use driver::driver::stop_after_phase_5;

    // We use separate paths for PNaCl targets so we can save LLVM
    // attributes in our module. Otherwise, we'd have to discard them
    // for 'assembly' (llmod -> .o bitcode using pnacl-clang).

    // Rlibs and statics first, then exes.
    for &crate_type in sess.crate_types.borrow().iter() {
        match crate_type {
            config::CrateTypeDylib => {
                debug!("skipping dylib output; PNaCl doesn't support \
                       it yet");
            }
            config::CrateTypeRlib => link_pnacl_rlib(sess, trans, outputs, &id),
            config::CrateTypeExecutable => {}
            config::CrateTypeStaticlib => unimplemented!(),
        }
    }
    if stop_after_phase_5(sess) { return; }
    for &crate_type in sess.crate_types.borrow().iter() {
        match crate_type {
            config::CrateTypeExecutable => link_pnacl_module(sess, trans,
                                                             outputs, &id),
            _ => (),
        }
    }
}

fn search_for_native(paths: &Vec<Path>,
                     name: &String) -> Option<Path> {
    use std::os::consts::{DLL_PREFIX};
    use std::io::fs::stat;
    let name_path = Path::new(format!("{}{}.a", DLL_PREFIX, name));
    debug!(">> searching for native lib `{}` starting", name);
    let found = paths.iter().find(|dir| {
        debug!("   searching in `{}`", dir.display());
        match stat(&dir.join(&name_path)) {
            Ok(_) => true,
            Err(_) => false,
        }
    });
    let found = found.map(|f| f.join(&name_path) );
    debug!("<< searching for native lib `{}` finished, result=`{}`",
           name, found.as_ref().map(|f| f.display() ));
    found
}
fn maybe_lib<T>(sess: &Session, name: &String, p_opt: Option<T>) -> Option<T> {
    p_opt.or_else(|| {
        sess.err(format!("couldn't find library `{}`", name).as_slice());
        sess.note("maybe missing `-L`?");
        None
    })
}

fn pnacl_lib_paths(sess: &Session) -> Vec<Path> {
    use std::os;
    let native_dep_lib_path = Some({
        os::make_absolute(&sess.pnacl_toolchain()
                          .join("lib"))
    });
    let native_dep_sdk_lib_path = Some({
        os::make_absolute(&sess.pnacl_toolchain()
                          .join("sdk")
                          .join("lib"))
    });
    let native_dep_usr_lib_path = Some({
        os::make_absolute(&sess.pnacl_toolchain()
                          .join("usr")
                          .join("lib"))
    });
    let ports_lib_path = Some({
        os::make_absolute(&sess.expect_cross_path()
                          .join("lib")
                          .join("pnacl")
                          .join(if sess.opts.optimize == config::No {
                              "Debug"
                          } else {
                              "Release"
                          }))
    });
    let addl_lib_paths = sess.opts.addl_lib_search_paths.borrow();
    let iter = addl_lib_paths.iter();
    let iter = iter.chain(native_dep_lib_path.iter());
    let iter = iter.chain(native_dep_sdk_lib_path.iter());
    let iter = iter.chain(native_dep_usr_lib_path.iter());
    let iter = iter.chain(ports_lib_path.iter());
    let mut iter = iter.map(|p| p.clone() );
    iter.collect()
}

fn already_linked_libs(sess: &Session, crates: &Vec<(u32, Option<Path>)>) -> HashSet<String> {
    // Go though all extern crates and insert their deps into our linked set.
    let linked = crates
        .iter()
        .fold(HashSet::new(), |mut led: HashSet<String>, &(cnum, _)| {
            let libs = csearch::get_native_libraries(&sess.cstore, cnum);
            for (_, name) in libs.move_iter() {
                led.insert(name);
            }
            led
        });
    debug!("linked: `{}`", linked);
    linked
}
fn link_attrs_filter(sess: &Session,
                     lib: &String,
                     kind: cstore::NativeLibaryKind,
                     linked: &mut HashSet<String>,
                     lib_paths: &Vec<Path>) -> Option<(String, Path)> {
    match kind {
        cstore::NativeFramework => {
            sess.bug("can't link MacOS frameworks into PNaCl modules");
        }
        cstore::NativeStatic | cstore::NativeUnknown
            if !linked.contains(lib) => {
                // Don't link archives twice:
                linked.insert(lib.clone());
                let lib_path = search_for_native(lib_paths, lib);
                maybe_lib(sess, lib, lib_path.map(|p| (lib.clone(), p) ))
            }
        cstore::NativeStatic | cstore::NativeUnknown => { None }
    }
}

pub fn link_pnacl_module(sess: &Session,
                         trans: &mut CrateTranslation,
                         outputs: &OutputFilenames,
                         id: &CrateId) {
    // Note that we don't use pnacl-ld here. We want to avoid the costly post-link
    // simplification passes pnacl-ld runs. Instead, since pnacl-ld's output is just bitcode,
    // what we do here is pull all of our 'native' dependencies into a (vary large)
    // module and run the LTO passes on it. Admittedly some-what hacky.
    //
    // To save time, this is done as a three pass LTO of sorts: the first one pulls all
    // the native libs/bcs deps (things like crti.bc, libc.a, libnacl.a, object files in
    // rlibs) into our module. We then run our ABI simplification passes and optimizations,
    // minus the EH passes. Then, we link in the std crates (libstd, libcollections, etc),
    // which have already been (mostly) simplified for PNaCl's ABI. Lastly, we run the EH
    // passes on the whole caboodle.
    //
    // Naturally, #[link_args] get ignored. However, #[link]s still need to be handled,
    // hence we have to support searching for these libs.
    //
    // For crates that use both bin & some other crate_type, we will be running on an
    // already optimized module. So technically we'll be running the optimization passes
    // twice. Sad. FIXME. Can be done by gathering all deps into one module, running
    // passes, and then joining the module with the crate's module.
    //
    // This function should not be called on non-exe outputs.
    use libc;
    use lib::llvm::{ModuleRef, ContextRef};
    use back::lto;
    use std::io::File;

    let llmod = trans.module;

    fn link_buf_into_module(sess: &Session,
                            ctxt:  ContextRef,
                            llmod: Option<ModuleRef>,
                            name: &str,
                            bc: &[u8]) -> Option<ModuleRef> {
        use libc;
        debug!("inserting `{}` into module", name);
        match llmod {
            Some(llmod) => unsafe {
                if !llvm::LLVMRustLinkInExternalBitcode(llmod,
                                                        bc.as_ptr() as *libc::c_char,
                                                        bc.len() as libc::size_t) {
                    llvm_warn(sess, format!("failed to link in external bitcode `{}`",
                                            name));
                }
                Some(llmod)
            },
            None => unsafe {
                let llmod = name.with_c_str(|s| {
                    llvm::LLVMRustParseBitcode(ctxt,
                                               s,
                                               bc.as_ptr() as *libc::c_void,
                                               bc.len() as libc::size_t)
                });
                if llmod == ptr::null() {
                    llvm_warn(sess, format!("failed to parse external bitcode `{}`",
                                            name));
                    None
                } else {
                    Some(llmod)
                }
            },
        }
    }

    let save_temp = |ext, llmod| {
        if sess.opts.cg.save_temps {
            outputs.with_extension(ext).with_c_str(|buf| unsafe {
                llvm::LLVMWriteBitcodeToFile(llmod, buf);
            })
        }
    };

    // FIXME: Now that objects created with pnacl-clang are linked into a
    // separate mod, there's no reason we can't attach bitcodes directly to
    // whatever rlib uses them and run the PNaCl ABI passes on them once when
    // they're built.
    // Hopefully, once implemented this will finally bring the link time down
    // enough for PNaCl crates that running the r-pass checks can complete in
    // our lifetime.

    // # Pull all 'native' deps into our module:
    // ## support bitcodes:
    debug!("linking support bitcodes");
    {
        let bcs = vec!("crti.bc",
                       "crtbegin.bc",
                       "sjlj_eh_redirect.bc");
        let mut iter = bcs
            .iter()
            .zip(bcs.iter()
                 .map(|bc| {
                     sess.pnacl_toolchain()
                         .join("lib")
                         .join(bc.clone())
                 })
                 .filter_map(|bc_path| {
                     // load the bitcode into memory:
                     time(sess.time_passes(),
                          format!("reading `{}`", bc_path.display()).as_slice(),
                          (),
                          |()| match File::open(&bc_path).read_to_end() {
                              Ok(buf) => Some(buf),
                              Err(e) => {
                                  sess.err(format!("error reading file `{}`: `{}`",
                                                   bc_path.display(), e).as_slice());
                                  None
                              }
                          } )
                 }));
        let tc_mod = iter.fold(None, |m, (name, bc)| {
            let bc: Vec<u8> = bc;
            link_buf_into_module(sess,
                                 trans.context,
                                 m,
                                 *name,
                                 bc.as_slice())
        });

        let tc_mod = sess.opts.cg.extra_bitcode
            .iter()
            .map(|file| Path::new(file.clone()) )
            .fold(tc_mod, |m, extra| {
                match File::open(&extra).read_to_end() {
                    Ok(buf) => link_buf_into_module(sess,
                                                    trans.context,
                                                    m,
                                                    extra.display().to_str().as_slice(),
                                                    buf.as_slice()),
                    Err(e) => {
                        sess.err(format!("error reading file `{}`: `{}`",
                                         extra.display(), e).as_slice());
                        m
                    }
                }
            });
        sess.abort_if_errors();

        // Merge the toolchain module and our module together.
        // This also prevents our LLVM tools from stripping debugging info from our
        // module.
        save_temp("tc-pre-debug-purge.bc", *tc_mod.get_ref());
        unsafe {
            llvm::LLVMRustStripDebugInfo(*tc_mod.get_ref());
            save_temp("tc-post-debug-purge.bc", *tc_mod.get_ref());
            if !llvm::LLVMRustLinkInModule(llmod, tc_mod.unwrap()) {
                llvm_err(sess,
                         "failed to merge our translated mod with the toolchain mod".to_string());
            }
        }
    }

    // Some globals in the bitcode from PNaCl have what is considered invalid
    // linkage in our LLVM (their LLVM is old). Fortunately, all linkage types
    // get stripped later, so it's safe to just ignore them all.
    write::run_passes_on_mod(sess,
                             trans,
                             llmod,
                             outputs,
                             false,
                             true);

    if sess.opts.cg.save_temps {
        outputs.with_extension("post-opt.bc").with_c_str(|buf| unsafe {
            llvm::LLVMWriteBitcodeToFile(llmod, buf);
        })
    }

    let pre_link_path = outputs.with_extension("pre-link.bc");
    pre_link_path.with_c_str(|buf| unsafe {
        llvm::LLVMWriteBitcodeToFile(llmod, buf);
        llvm::LLVMDisposeModule(trans.module);
    });
    trans.module = ptr::null();
    
    let post_link_path = outputs.with_extension("post-link.bc");

    let linker = pnacl_host_tool(sess, "ld.gold");
    let mut cmd = Command::new(&linker);
    cmd.arg("--oformat=elf32-i386-nacl");
    cmd.arg(format!("-plugin={}", sess.gold_plugin_path().display()));
    cmd.arg("-plugin-opt=emit-llvm");
    cmd.arg("-nostdlib");
    cmd.arg("--undef-sym-check");
    cmd.arg("-static");
    cmd.args(["--allow-unresolved=memcpy",
              "--allow-unresolved=memset",
              "--allow-unresolved=memmove",
              "--allow-unresolved=setjmp",
              "--allow-unresolved=longjmp",
              "--allow-unresolved=__nacl_tp_tls_offset",
              "--allow-unresolved=__nacl_tp_tdb_offset",
              "--allow-unresolved=__nacl_get_arch",
              "--undefined=__pnacl_eh_stack",
              "--undefined=__pnacl_eh_resume",
              "--allow-unresolved=__pnacl_eh_type_table",
              "--allow-unresolved=__pnacl_eh_action_table",
              "--allow-unresolved=__pnacl_eh_filter_table",
              "--undefined=main",
              "--undefined=exit",
              "--undefined=_exit",
              ]);
    cmd.arg(&pre_link_path);
    cmd.arg("-o").arg(&post_link_path);

    cmd.arg("-L").arg(sess.target_filesearch().get_lib_path());

    cmd.args(sess.opts.cg.link_args.as_slice());
    for arg in sess.cstore.get_used_link_args().borrow().iter() {
        cmd.arg(arg.as_slice());
    }
    cmd.arg("--start-group");
    let deps = sess.cstore.get_used_crates(cstore::RequireStatic);
    for &(cnum, _) in deps.iter() {
        let src = sess.cstore.get_used_crate_source(cnum).unwrap();
        cmd.arg(src.rlib.unwrap());
    }
    cmd.arg("--end-group");

    debug!("running toolchain linker:");
    debug!("{}", &cmd);
    let prog = time(sess.time_passes(),
                    "running linker",
                    (),
                    |()| cmd.output() );
    match prog {
        Ok(prog) => {
            if !prog.status.success() {
                sess.err(format!("linking with `{}` failed: {}",
                                 linker.display(),
                                 prog.status).as_slice());
                sess.note(format!("{}", &cmd).as_slice());
                let mut output = prog.error.clone();
                output.push_all(prog.output.as_slice());
                sess.note(str::from_utf8(output.as_slice()).unwrap()
                                                           .as_slice());
                sess.abort_if_errors();
            }
        },
        Err(e) => {
            sess.err(format!("could not exec the linker `{}`: {}",
                             linker.display(),
                             e).as_slice());
            sess.abort_if_errors();
        }
    }

    if !sess.opts.cg.save_temps {
        remove(sess, &pre_link_path);
    }

    // Now load the linked module and run LTO + a limited set of passes to cleanup:
    trans.module = match File::open(&post_link_path).read_to_end() {
        Ok(buf) => link_buf_into_module(sess,
                                        trans.context,
                                        None,
                                        post_link_path.display().as_maybe_owned().as_slice(),
                                        buf.as_slice()).unwrap(),
        Err(e) => {
            sess.fatal(format!("error reading file `{}`: `{}`",
                               post_link_path.display(), e).as_slice());
        }
    };
    let llmod = trans.module;

    if !sess.opts.cg.save_temps {
        remove(sess, &post_link_path);
    }

    // # Run LTO passes:

    // Internalize everything.
    unsafe {
        let start = "_start".to_c_str();
        let reachable = vec!(start.with_ref(|p| p ));
        llvm::LLVMRustRunRestrictionPass(llmod,
                                         reachable.as_ptr() as **libc::c_char,
                                         reachable.len() as libc::size_t);
    }

    if sess.no_landing_pads() {
        unsafe {
            llvm::LLVMRustMarkAllFunctionsNounwind(llmod);
        }
    }

    if sess.opts.cg.save_temps {
        outputs.with_extension("pre-lto.bc").with_c_str(|buf| unsafe {
            llvm::LLVMWriteBitcodeToFile(llmod, buf);
        })
    }

    // EH pass + final LTO:
    lto::run_passes(sess,
                    llmod,
                    ptr::null(),
                    |pm| {
                        let ap = |s| {
                            unsafe {
                                assert!(llvm::LLVMRustAddPass(pm, s));
                            }
                        };
                        "nacl-expand-tls".with_c_str(|s| ap(s) );
                        "nacl-global-cleanup".with_c_str(|s| ap(s) );
                    },
                    |pm| {
                        let ap = |s| {
                            unsafe {
                                assert!(llvm::LLVMRustAddPass(pm, s));
                            }
                        };
                        "resolve-aliases".with_c_str(|s| ap(s) );
                        // Now cleanup the optimizations:
                        "expand-constant-expr".with_c_str(|s| ap(s) );
                        "flatten-globals".with_c_str(|s| ap(s) );
                        "replace-ptrs-with-ints".with_c_str(|s| ap(s) );
                        "nacl-promote-ints".with_c_str(|s| ap(s) );
                        "nacl-promote-i1-ops".with_c_str(|s| ap(s) );
                        "expand-getelementptr".with_c_str(|s| ap(s) );
                        "canonicalize-mem-intrinsics".with_c_str(|s| ap(s) );

                        "nacl-strip-attributes".with_c_str(|s| ap(s) );
                        "strip-dead-prototypes".with_c_str(|s| ap(s) );
                        // Strip unsupported metadata:
                        "strip-metadata".with_c_str(|s| ap(s) );
                        if !sess.no_verify() {
                            "verify-pnaclabi-module".with_c_str(|s| ap(s) );
                            "verify-pnaclabi-functions".with_c_str(|s| ap(s) );
                        }
                    });

    // Write out IR if asked:
    if sess.opts.output_types.iter().any(|&i| i == OutputTypeLlvmAssembly) {
        // emit ir
        let p = outputs.path(OutputTypeLlvmAssembly);
        unsafe {
            let pm = llvm::LLVMCreatePassManager();
            p.with_c_str(|output| {
                llvm::LLVMRustPrintModule(pm, llmod, output);
            });
            llvm::LLVMDisposePassManager(pm);
        }
    }
    let out = match outputs.single_output_file {
        Some(ref file) => file.clone(),
        None => {
            let out_filename = outputs.path(OutputTypeExe);
            filename_for_input(sess, config::CrateTypeExecutable, id, &out_filename)
        }
    };

    if sess.opts.cg.stable_pexe {
        // stable output:
        if sess.opts.cg.save_temps {
            outputs.with_extension("final.bc").with_c_str(|buf| unsafe {
                llvm::LLVMWriteBitcodeToFile(llmod, buf);
            })
        }
        sess.check_writeable_output(&out, "final output");
        out.with_c_str(|output| unsafe {
            if !llvm::LLVMRustWritePNaClBitcode(llmod, output, false) {
                llvm_err(sess, "failed to write output file".to_string());
            }
        });
    } else {
        sess.check_writeable_output(&out, "final output");
        // regular bitcode output:
        out.with_c_str(|buf| unsafe {
            llvm::LLVMWriteBitcodeToFile(llmod, buf);
        })
    }
}

// Create an 'rlib'
//
// An rlib in its current incarnation is essentially a renamed .a file. The
// rlib primarily contains the object file of the crate, but it also contains
// all of the object files from native libraries. This is done by unzipping
// native libraries and inserting all of the contents into this archive.
fn link_rlib<'a>(sess: &'a Session,
                 trans: Option<&CrateTranslation>, // None == no metadata/bytecode
                 obj_filename: &Path,
                 out_filename: &Path) -> Archive<'a> {
    let mut a = Archive::create(sess, out_filename, obj_filename);

    for &(ref l, kind) in sess.cstore.get_used_libraries().borrow().iter() {
        match kind {
            cstore::NativeStatic => {
                a.add_native_library(l.as_slice()).unwrap();
            }
            cstore::NativeFramework | cstore::NativeUnknown => {}
        }
    }

    // Note that it is important that we add all of our non-object "magical
    //
    // * When performing LTO, this archive will be modified to remove
    //   obj_filename from above. The reason for this is described below.
    //
    // * When the system linker looks at an archive, it will attempt to
    //   determine the architecture of the archive in order to see whether its
    //   linkable.
    //
    //   The algorithm for this detection is: iterate over the files in the
    //   archive. Skip magical SYMDEF names. Interpret the first file as an
    //   object file. Read architecture from the object file.
    //
    // * As one can probably see, if "metadata" and "foo.bc" were placed
    //   before all of the objects, then the architecture of this archive would
    //   not be correctly inferred once 'foo.o' is removed.
    //
    // Basically, all this means is that this code should not move above the
    // code above.
    match trans {
        Some(trans) => {
            // Instead of putting the metadata in an object file section, rlibs
            // contain the metadata in a separate file. We use a temp directory
            // here so concurrent builds in the same directory don't try to use
            // the same filename for metadata (stomping over one another)
            let tmpdir = TempDir::new("rustc").expect("needs a temp dir");
            let metadata = tmpdir.path().join(METADATA_FILENAME);
            match fs::File::create(&metadata).write(trans.metadata
                                                         .as_slice()) {
                Ok(..) => {}
                Err(e) => {
                    sess.err(format!("failed to write {}: {}",
                                     metadata.display(),
                                     e).as_slice());
                    sess.abort_if_errors();
                }
            }
            a.add_file(&metadata, false);
            remove(sess, &metadata);

            // For LTO purposes, the bytecode of this library is also inserted
            // into the archive.
            //
            // Note that we make sure that the bytecode filename in the archive
            // is never exactly 16 bytes long by adding a 16 byte extension to
            // it. This is to work around a bug in LLDB that would cause it to
            // crash if the name of a file in an archive was exactly 16 bytes.
            let bc = obj_filename.with_extension("bc");
            let bc_deflated = obj_filename.with_extension("bytecode.deflate");
            match fs::File::open(&bc).read_to_end().and_then(|data| {
                fs::File::create(&bc_deflated)
                    .write(match flate::deflate_bytes(data.as_slice()) {
                        Some(compressed) => compressed,
                        None => sess.fatal("failed to compress bytecode")
                     }.as_slice())
            }) {
                Ok(()) => {}
                Err(e) => {
                    sess.err(format!("failed to write compressed bytecode: \
                                      {}",
                                     e).as_slice());
                    sess.abort_if_errors()
                }
            }
            a.add_file(&bc_deflated, false);
            remove(sess, &bc_deflated);
            if !sess.opts.cg.save_temps &&
               !sess.opts.output_types.contains(&OutputTypeBitcode) {
                remove(sess, &bc);
            }

            // After adding all files to the archive, we need to update the
            // symbol table of the archive. This currently dies on OSX (see
            // #11162), and isn't necessary there anyway
            match sess.targ_cfg.os {
                abi::OsMacos => {}
                _ => { a.update_symbols(); }
            }
        }

        None => {}
    }
    return a;
}

// Create a static archive
//
// This is essentially the same thing as an rlib, but it also involves adding
// all of the upstream crates' objects into the archive. This will slurp in
// all of the native libraries of upstream dependencies as well.
//
// Additionally, there's no way for us to link dynamic libraries, so we warn
// about all dynamic library dependencies that they're not linked in.
//
// There's no need to include metadata in a static archive, so ensure to not
// link in the metadata object file (and also don't prepare the archive with a
// metadata file).
fn link_staticlib(sess: &Session, obj_filename: &Path, out_filename: &Path) {
    let mut a = link_rlib(sess, None, obj_filename, out_filename);
    if !sess.targeting_pnacl() {
        a.add_native_library("morestack").unwrap();
    }
    a.add_native_library("compiler-rt").unwrap();

    let crates = sess.cstore.get_used_crates(cstore::RequireStatic);
    let mut all_native_libs = vec![];

    for &(cnum, ref path) in crates.iter() {
        let name = sess.cstore.get_crate_data(cnum).name.clone();
        let p = match *path {
            Some(ref p) => p.clone(), None => {
                sess.err(format!("could not find rlib for: `{}`",
                                 name).as_slice());
                continue
            }
        };
        a.add_rlib(&p, name.as_slice(), sess.lto()).unwrap();

        let native_libs = csearch::get_native_libraries(&sess.cstore, cnum);
        all_native_libs.extend(native_libs.move_iter());
    }

    if !all_native_libs.is_empty() {
        sess.warn("link against the following native artifacts when linking against \
                  this static library");
        sess.note("the order and any duplication can be significant on some platforms, \
                  and so may need to be preserved");
    }

    for &(kind, ref lib) in all_native_libs.iter() {
        let name = match kind {
            cstore::NativeStatic => "static library",
            cstore::NativeUnknown => "library",
            cstore::NativeFramework => "framework",
        };
        sess.note(format!("{}: {}", name, *lib).as_slice());
    }
}

// Create a dynamic library or executable
//
// This will invoke the system linker/cc to create the resulting file. This
// links to all upstream files as well.
fn link_natively(sess: &Session, trans: &CrateTranslation, dylib: bool,
                 obj_filename: &Path, out_filename: &Path) {
    let tmpdir = TempDir::new("rustc").expect("needs a temp dir");

    // The invocations of cc share some flags across platforms
    let pname = get_cc_prog(sess);
    let mut cmd = Command::new(pname.as_slice());

    cmd.args(sess.targ_cfg.target_strs.cc_args.as_slice());
    link_args(&mut cmd, sess, dylib, tmpdir.path(),
              trans, obj_filename, out_filename);

    if (sess.opts.debugging_opts & config::PRINT_LINK_ARGS) != 0 {
        println!("{}", &cmd);
    }

    // May have not found libraries in the right formats.
    sess.abort_if_errors();

    // Invoke the system linker
    debug!("{}", &cmd);
    let prog = time(sess.time_passes(), "running linker", (), |()| cmd.output());
    match prog {
        Ok(prog) => {
            if !prog.status.success() {
                sess.err(format!("linking with `{}` failed: {}",
                                 pname,
                                 prog.status).as_slice());
                sess.note(format!("{}", &cmd).as_slice());
                let mut output = prog.error.clone();
                output.push_all(prog.output.as_slice());
                sess.note(str::from_utf8(output.as_slice()).unwrap()
                                                           .as_slice());
                sess.abort_if_errors();
            }
        },
        Err(e) => {
            sess.err(format!("could not exec the linker `{}`: {}",
                             pname,
                             e).as_slice());
            sess.abort_if_errors();
        }
    }


    // On OSX, debuggers need this utility to get run to do some munging of
    // the symbols
    if sess.targ_cfg.os == abi::OsMacos && (sess.opts.debuginfo != NoDebugInfo) {
        match Command::new("dsymutil").arg(out_filename).status() {
            Ok(..) => {}
            Err(e) => {
                sess.err(format!("failed to run dsymutil: {}", e).as_slice());
                sess.abort_if_errors();
            }
        }
    }
}

fn link_args(cmd: &mut Command,
             sess: &Session,
             dylib: bool,
             tmpdir: &Path,
             trans: &CrateTranslation,
             obj_filename: &Path,
             out_filename: &Path) {

    // The default library location, we need this to find the runtime.
    // The location of crates will be determined as needed.
    let lib_path = sess.target_filesearch().get_lib_path();
    cmd.arg("-L").arg(lib_path);

    cmd.arg("-o").arg(out_filename).arg(obj_filename);

    // Stack growth requires statically linking a __morestack function. Note
    // that this is listed *before* all other libraries, even though it may be
    // used to resolve symbols in other libraries. The only case that this
    // wouldn't be pulled in by the object file is if the object file had no
    // functions.
    //
    // If we're building an executable, there must be at least one function (the
    // main function), and if we're building a dylib then we don't need it for
    // later libraries because they're all dylibs (not rlibs).
    //
    // I'm honestly not entirely sure why this needs to come first. Apparently
    // the --as-needed flag above sometimes strips out libstd from the command
    // line, but inserting this farther to the left makes the
    // "rust_stack_exhausted" symbol an outstanding undefined symbol, which
    // flags libstd as a required library (or whatever provides the symbol).
    cmd.arg("-lmorestack");

    // When linking a dynamic library, we put the metadata into a section of the
    // executable. This metadata is in a separate object file from the main
    // object file, so we link that in here.
    if dylib {
        cmd.arg(obj_filename.with_extension("metadata.o"));
    }

    // We want to prevent the compiler from accidentally leaking in any system
    // libraries, so we explicitly ask gcc to not link to any libraries by
    // default. Note that this does not happen for windows because windows pulls
    // in some large number of libraries and I couldn't quite figure out which
    // subset we wanted.
    //
    // FIXME(#11937) we should invoke the system linker directly
    if sess.targ_cfg.os != abi::OsWin32 && !sess.targeting_pnacl() {
        cmd.arg("-nodefaultlibs");
    }

    // If we're building a dylib, we don't use --gc-sections because LLVM has
    // already done the best it can do, and we also don't want to eliminate the
    // metadata. If we're building an executable, however, --gc-sections drops
    // the size of hello world from 1.8MB to 597K, a 67% reduction.
    if !dylib && sess.targ_cfg.os != abi::OsMacos {
        cmd.arg("-Wl,--gc-sections");
    }

    if sess.targ_cfg.os == abi::OsLinux {
        // GNU-style linkers will use this to omit linking to libraries which
        // don't actually fulfill any relocations, but only for libraries which
        // follow this flag. Thus, use it before specifying libraries to link to.
        cmd.arg("-Wl,--as-needed");

        // GNU-style linkers support optimization with -O. GNU ld doesn't need a
        // numeric argument, but other linkers do.
        if sess.opts.optimize == config::Default ||
           sess.opts.optimize == config::Aggressive {
            cmd.arg("-Wl,-O1");
        }
    } else if sess.targ_cfg.os == abi::OsMacos {
        // The dead_strip option to the linker specifies that functions and data
        // unreachable by the entry point will be removed. This is quite useful
        // with Rust's compilation model of compiling libraries at a time into
        // one object file. For example, this brings hello world from 1.7MB to
        // 458K.
        //
        // Note that this is done for both executables and dynamic libraries. We
        // won't get much benefit from dylibs because LLVM will have already
        // stripped away as much as it could. This has not been seen to impact
        // link times negatively.
        cmd.arg("-Wl,-dead_strip");
    }

    if sess.targ_cfg.os == abi::OsWin32 {
        // Make sure that we link to the dynamic libgcc, otherwise cross-module
        // DWARF stack unwinding will not work.
        // This behavior may be overridden by --link-args "-static-libgcc"
        cmd.arg("-shared-libgcc");

        // And here, we see obscure linker flags #45. On windows, it has been
        // found to be necessary to have this flag to compile liblibc.
        //
        // First a bit of background. On Windows, the file format is not ELF,
        // but COFF (at least according to LLVM). COFF doesn't officially allow
        // for section names over 8 characters, apparently. Our metadata
        // section, ".note.rustc", you'll note is over 8 characters.
        //
        // On more recent versions of gcc on mingw, apparently the section name
        // is *not* truncated, but rather stored elsewhere in a separate lookup
        // table. On older versions of gcc, they apparently always truncated the
        // section names (at least in some cases). Truncating the section name
        // actually creates "invalid" objects [1] [2], but only for some
        // introspection tools, not in terms of whether it can be loaded.
        //
        // Long story short, passing this flag forces the linker to *not*
        // truncate section names (so we can find the metadata section after
        // it's compiled). The real kicker is that rust compiled just fine on
        // windows for quite a long time *without* this flag, so I have no idea
        // why it suddenly started failing for liblibc. Regardless, we
        // definitely don't want section name truncation, so we're keeping this
        // flag for windows.
        //
        // [1] - https://sourceware.org/bugzilla/show_bug.cgi?id=13130
        // [2] - https://code.google.com/p/go/issues/detail?id=2139
        cmd.arg("-Wl,--enable-long-section-names");
    }

    if sess.targ_cfg.os == abi::OsAndroid {
        // Many of the symbols defined in compiler-rt are also defined in libgcc.
        // Android linker doesn't like that by default.
        cmd.arg("-Wl,--allow-multiple-definition");
    }

    // Take careful note of the ordering of the arguments we pass to the linker
    // here. Linkers will assume that things on the left depend on things to the
    // right. Things on the right cannot depend on things on the left. This is
    // all formally implemented in terms of resolving symbols (libs on the right
    // resolve unknown symbols of libs on the left, but not vice versa).
    //
    // For this reason, we have organized the arguments we pass to the linker as
    // such:
    //
    //  1. The local object that LLVM just generated
    //  2. Upstream rust libraries
    //  3. Local native libraries
    //  4. Upstream native libraries
    //
    // This is generally fairly natural, but some may expect 2 and 3 to be
    // swapped. The reason that all native libraries are put last is that it's
    // not recommended for a native library to depend on a symbol from a rust
    // crate. If this is the case then a staticlib crate is recommended, solving
    // the problem.
    //
    // Additionally, it is occasionally the case that upstream rust libraries
    // depend on a local native library. In the case of libraries such as
    // lua/glfw/etc the name of the library isn't the same across all platforms,
    // so only the consumer crate of a library knows the actual name. This means
    // that downstream crates will provide the #[link] attribute which upstream
    // crates will depend on. Hence local native libraries are after out
    // upstream rust crates.
    //
    // In theory this means that a symbol in an upstream native library will be
    // shadowed by a local native library when it wouldn't have been before, but
    // this kind of behavior is pretty platform specific and generally not
    // recommended anyway, so I don't think we're shooting ourself in the foot
    // much with that.
    add_upstream_rust_crates(cmd, sess, dylib, tmpdir, trans);
    add_local_native_libraries(cmd, sess);
    add_upstream_native_libraries(cmd, sess);

    // # Telling the linker what we're doing

    if dylib {
        // On mac we need to tell the linker to let this library be rpathed
        if sess.targ_cfg.os == abi::OsMacos {
            cmd.args(["-dynamiclib", "-Wl,-dylib"]);

            if !sess.opts.cg.no_rpath {
                let mut v = Vec::from_slice("-Wl,-install_name,@rpath/".as_bytes());
                v.push_all(out_filename.filename().unwrap());
                cmd.arg(v.as_slice());
            }
        } else {
            cmd.arg("-shared");
        }
    }

    if sess.targ_cfg.os == abi::OsFreebsd {
        cmd.args(["-L/usr/local/lib",
                  "-L/usr/local/lib/gcc46",
                  "-L/usr/local/lib/gcc44"]);
    }

    // FIXME (#2397): At some point we want to rpath our guesses as to
    // where extern libraries might live, based on the
    // addl_lib_search_paths
    if !sess.opts.cg.no_rpath {
        cmd.args(rpath::get_rpath_flags(sess, out_filename).as_slice());
    }

    // compiler-rt contains implementations of low-level LLVM helpers. This is
    // used to resolve symbols from the object file we just created, as well as
    // any system static libraries that may be expecting gcc instead. Most
    // symbols in libgcc also appear in compiler-rt.
    //
    // This is the end of the command line, so this library is used to resolve
    // *all* undefined symbols in all other libraries, and this is intentional.
    cmd.arg("-lcompiler-rt");

    // Finally add all the linker arguments provided on the command line along
    // with any #[link_args] attributes found inside the crate
    cmd.args(sess.opts.cg.link_args.as_slice());
    for arg in sess.cstore.get_used_link_args().borrow().iter() {
        cmd.arg(arg.as_slice());
    }
}

// # Native library linking
//
// User-supplied library search paths (-L on the command line). These are
// the same paths used to find Rust crates, so some of them may have been
// added already by the previous crate linking code. This only allows them
// to be found at compile time so it is still entirely up to outside
// forces to make sure that library can be found at runtime.
//
// Also note that the native libraries linked here are only the ones located
// in the current crate. Upstream crates with native library dependencies
// may have their native library pulled in above.
fn add_local_native_libraries(cmd: &mut Command, sess: &Session) {
    for path in sess.opts.addl_lib_search_paths.borrow().iter() {
        cmd.arg("-L").arg(path);
    }

    let rustpath = filesearch::rust_path();
    for path in rustpath.iter() {
        cmd.arg("-L").arg(path);
    }

    // Some platforms take hints about whether a library is static or dynamic.
    // For those that support this, we ensure we pass the option if the library
    // was flagged "static" (most defaults are dynamic) to ensure that if
    // libfoo.a and libfoo.so both exist that the right one is chosen.
    let takes_hints = sess.targ_cfg.os != abi::OsMacos;

    for &(ref l, kind) in sess.cstore.get_used_libraries().borrow().iter() {
        match kind {
            cstore::NativeUnknown | cstore::NativeStatic => {
                if takes_hints {
                    if kind == cstore::NativeStatic {
                        cmd.arg("-Wl,-Bstatic");
                    } else {
                        cmd.arg("-Wl,-Bdynamic");
                    }
                }
                cmd.arg(format!("-l{}", *l));
            }
            cstore::NativeFramework => {
                cmd.arg("-framework");
                cmd.arg(l.as_slice());
            }
        }
    }
    if takes_hints {
        cmd.arg("-Wl,-Bdynamic");
    }
}

// # Rust Crate linking
//
// Rust crates are not considered at all when creating an rlib output. All
// dependencies will be linked when producing the final output (instead of
// the intermediate rlib version)
fn add_upstream_rust_crates(cmd: &mut Command, sess: &Session,
                            dylib: bool, tmpdir: &Path,
                            trans: &CrateTranslation) {
    // All of the heavy lifting has previously been accomplished by the
    // dependency_format module of the compiler. This is just crawling the
    // output of that module, adding crates as necessary.
    //
    // Linking to a rlib involves just passing it to the linker (the linker
    // will slurp up the object files inside), and linking to a dynamic library
    // involves just passing the right -l flag.

    let data = if dylib {
        trans.crate_formats.get(&config::CrateTypeDylib)
    } else {
        trans.crate_formats.get(&config::CrateTypeExecutable)
    };

    // Invoke get_used_crates to ensure that we get a topological sorting of
    // crates.
    let deps = sess.cstore.get_used_crates(cstore::RequireDynamic);

    for &(cnum, _) in deps.iter() {
        // We may not pass all crates through to the linker. Some crates may
        // appear statically in an existing dylib, meaning we'll pick up all the
        // symbols from the dylib.
        let kind = match *data.get(cnum as uint - 1) {
            Some(t) => t,
            None => continue
        };
        let src = sess.cstore.get_used_crate_source(cnum).unwrap();
        match kind {
            cstore::RequireDynamic => {
                add_dynamic_crate(cmd, sess, src.dylib.unwrap())
            }
            cstore::RequireStatic => {
                add_static_crate(cmd, sess, tmpdir, cnum, src.rlib.unwrap())
            }
        }

    }

    // Converts a library file-stem into a cc -l argument
    fn unlib<'a>(config: &config::Config, stem: &'a [u8]) -> &'a [u8] {
        if stem.starts_with("lib".as_bytes()) && config.os != abi::OsWin32 {
            stem.tailn(3)
        } else {
            stem
        }
    }

    // Adds the static "rlib" versions of all crates to the command line.
    fn add_static_crate(cmd: &mut Command, sess: &Session, tmpdir: &Path,
                        cnum: ast::CrateNum, cratepath: Path) {
        // When performing LTO on an executable output, all of the
        // bytecode from the upstream libraries has already been
        // included in our object file output. We need to modify all of
        // the upstream archives to remove their corresponding object
        // file to make sure we don't pull the same code in twice.
        //
        // We must continue to link to the upstream archives to be sure
        // to pull in native static dependencies. As the final caveat,
        // on linux it is apparently illegal to link to a blank archive,
        // so if an archive no longer has any object files in it after
        // we remove `lib.o`, then don't link against it at all.
        //
        // If we're not doing LTO, then our job is simply to just link
        // against the archive.
        if sess.lto() {
            let name = sess.cstore.get_crate_data(cnum).name.clone();
            time(sess.time_passes(),
                 format!("altering {}.rlib", name).as_slice(),
                 (), |()| {
                let dst = tmpdir.join(cratepath.filename().unwrap());
                match fs::copy(&cratepath, &dst) {
                    Ok(..) => {}
                    Err(e) => {
                        sess.err(format!("failed to copy {} to {}: {}",
                                         cratepath.display(),
                                         dst.display(),
                                         e).as_slice());
                        sess.abort_if_errors();
                    }
                }
                let mut archive = Archive::open(sess, dst.clone());
                archive.remove_file(format!("{}.o", name).as_slice());
                let files = archive.files();
                if files.iter().any(|s| s.as_slice().ends_with(".o")) {
                    cmd.arg(dst);
                }
            });
        } else {
            cmd.arg(cratepath);
        }
    }

    // Same thing as above, but for dynamic crates instead of static crates.
    fn add_dynamic_crate(cmd: &mut Command, sess: &Session, cratepath: Path) {
        // If we're performing LTO, then it should have been previously required
        // that all upstream rust dependencies were available in an rlib format.
        assert!(!sess.lto());

        // Just need to tell the linker about where the library lives and
        // what its name is
        let dir = cratepath.dirname();
        if !dir.is_empty() { cmd.arg("-L").arg(dir); }

        let mut v = Vec::from_slice("-l".as_bytes());
        v.push_all(unlib(&sess.targ_cfg, cratepath.filestem().unwrap()));
        cmd.arg(v.as_slice());
    }
}

// Link in all of our upstream crates' native dependencies. Remember that
// all of these upstream native dependencies are all non-static
// dependencies. We've got two cases then:
//
// 1. The upstream crate is an rlib. In this case we *must* link in the
// native dependency because the rlib is just an archive.
//
// 2. The upstream crate is a dylib. In order to use the dylib, we have to
// have the dependency present on the system somewhere. Thus, we don't
// gain a whole lot from not linking in the dynamic dependency to this
// crate as well.
//
// The use case for this is a little subtle. In theory the native
// dependencies of a crate are purely an implementation detail of the crate
// itself, but the problem arises with generic and inlined functions. If a
// generic function calls a native function, then the generic function must
// be instantiated in the target crate, meaning that the native symbol must
// also be resolved in the target crate.
fn add_upstream_native_libraries(cmd: &mut Command, sess: &Session) {
    // Be sure to use a topological sorting of crates because there may be
    // interdependencies between native libraries. When passing -nodefaultlibs,
    // for example, almost all native libraries depend on libc, so we have to
    // make sure that's all the way at the right (liblibc is near the base of
    // the dependency chain).
    //
    // This passes RequireStatic, but the actual requirement doesn't matter,
    // we're just getting an ordering of crate numbers, we're not worried about
    // the paths.
    let crates = sess.cstore.get_used_crates(cstore::RequireStatic);
    for (cnum, _) in crates.move_iter() {
        let libs = csearch::get_native_libraries(&sess.cstore, cnum);
        for &(kind, ref lib) in libs.iter() {
            match kind {
                cstore::NativeUnknown => {
                    cmd.arg(format!("-l{}", *lib));
                }
                cstore::NativeFramework => {
                    cmd.arg("-framework");
                    cmd.arg(lib.as_slice());
                }
                cstore::NativeStatic => {
                    sess.bug("statics shouldn't be propagated");
                }
            }
        }
    }
}
