// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use back::lto;
use back::link::{get_cc_prog, remove};
use session::config::{OutputFilenames, NoDebugInfo, Passes, SomePasses, AllPasses};
use session::Session;
use session::config;
use llvm;
use llvm::{ModuleRef, TargetMachineRef, PassManagerRef, DiagnosticInfoRef, ContextRef};
use llvm::SMDiagnosticRef;
use trans::{CrateTranslation, ModuleTranslation};
use util::common::time;
use syntax::codemap;
use syntax::diagnostic;
use syntax::diagnostic::{Emitter, Handler, Level, mk_handler};

use std::c_str::{ToCStr, CString};
use std::io::Command;
use std::io::fs;
use std::iter::Unfold;
use std::ptr;
use std::str;
use std::mem;
use std::sync::{Arc, Mutex};
use std::thread;
use libc::{c_uint, c_int, c_void};

#[deriving(Clone, Copy, PartialEq, PartialOrd, Ord, Eq)]
pub enum OutputType {
    OutputTypeBitcode,
    OutputTypeAssembly,
    OutputTypeLlvmAssembly,
    OutputTypeObject,
    OutputTypeExe,
}

pub fn llvm_err(handler: &diagnostic::Handler, msg: String) -> ! {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            handler.fatal(msg[]);
        } else {
            let err = CString::new(cstr, true);
            let err = String::from_utf8_lossy(err.as_bytes());
            handler.fatal(format!("{}: {}",
                                  msg[],
                                  err[])[]);
        }
    }
}

pub fn write_output_file(
        handler: &diagnostic::Handler,
        target: llvm::TargetMachineRef,
        pm: llvm::PassManagerRef,
        m: ModuleRef,
        output: &Path,
        file_type: llvm::FileType) {
    unsafe {
        output.with_c_str(|output| {
            let result = llvm::LLVMRustWriteOutputFile(
                    target, pm, m, output, file_type);
            if !result {
                llvm_err(handler, "could not write output".to_string());
            }
        })
    }
}


struct Diagnostic {
    msg: String,
    code: Option<String>,
    lvl: Level,
}

// We use an Arc instead of just returning a list of diagnostics from the
// child task because we need to make sure that the messages are seen even
// if the child task panics (for example, when `fatal` is called).
#[deriving(Clone)]
struct SharedEmitter {
    buffer: Arc<Mutex<Vec<Diagnostic>>>,
}

impl SharedEmitter {
    fn new() -> SharedEmitter {
        SharedEmitter {
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn dump(&mut self, handler: &Handler) {
        let mut buffer = self.buffer.lock();
        for diag in buffer.iter() {
            match diag.code {
                Some(ref code) => {
                    handler.emit_with_code(None,
                                           diag.msg[],
                                           code[],
                                           diag.lvl);
                },
                None => {
                    handler.emit(None,
                                 diag.msg[],
                                 diag.lvl);
                },
            }
        }
        buffer.clear();
    }
}

impl Emitter for SharedEmitter {
    fn emit(&mut self, cmsp: Option<(&codemap::CodeMap, codemap::Span)>,
            msg: &str, code: Option<&str>, lvl: Level) {
        assert!(cmsp.is_none(), "SharedEmitter doesn't support spans");

        self.buffer.lock().push(Diagnostic {
            msg: msg.to_string(),
            code: code.map(|s| s.to_string()),
            lvl: lvl,
        });
    }

    fn custom_emit(&mut self, _cm: &codemap::CodeMap,
                   _sp: diagnostic::RenderSpan, _msg: &str, _lvl: Level) {
        panic!("SharedEmitter doesn't support custom_emit");
    }
}


// On android, we by default compile for armv7 processors. This enables
// things like double word CAS instructions (rather than emulating them)
// which are *far* more efficient. This is obviously undesirable in some
// cases, so if any sort of target feature is specified we don't append v7
// to the feature list.
//
// On iOS only armv7 and newer are supported. So it is useful to
// get all hardware potential via VFP3 (hardware floating point)
// and NEON (SIMD) instructions supported by LLVM.
// Note that without those flags various linking errors might
// arise as some of intrinsics are converted into function calls
// and nobody provides implementations those functions
fn target_feature(sess: &Session) -> String {
    format!("{},{}", sess.target.target.options.features, sess.opts.cg.target_feature)
}

fn get_llvm_opt_level(optimize: config::OptLevel) -> llvm::CodeGenOptLevel {
    match optimize {
      config::No => llvm::CodeGenLevelNone,
      config::Less => llvm::CodeGenLevelLess,
      config::Default => llvm::CodeGenLevelDefault,
      config::Aggressive => llvm::CodeGenLevelAggressive,
    }
}

pub fn create_target_machine(sess: &Session) -> TargetMachineRef {
    let reloc_model_arg = match sess.opts.cg.relocation_model {
        Some(ref s) => s[],
        None => sess.target.target.options.relocation_model[]
    };
    let reloc_model = match reloc_model_arg {
        "pic" => llvm::RelocPIC,
        "static" => llvm::RelocStatic,
        "default" => llvm::RelocDefault,
        "dynamic-no-pic" => llvm::RelocDynamicNoPic,
        _ => {
            sess.err(format!("{} is not a valid relocation mode",
                             sess.opts
                                 .cg
                                 .relocation_model)[]);
            sess.abort_if_errors();
            unreachable!();
        }
    };

    let opt_level = get_llvm_opt_level(sess.opts.optimize);
    let use_softfp = sess.opts.cg.soft_float;

    // FIXME: #11906: Omitting frame pointers breaks retrieving the value of a parameter.
    let no_fp_elim = (sess.opts.debuginfo != NoDebugInfo) ||
                     !sess.target.target.options.eliminate_frame_pointer;

    let any_library = sess.crate_types.borrow().iter().any(|ty| {
        *ty != config::CrateTypeExecutable
    });

    let ffunction_sections = sess.target.target.options.function_sections;
    let fdata_sections = ffunction_sections;

    let code_model_arg = match sess.opts.cg.code_model {
        Some(ref s) => s[],
        None => sess.target.target.options.code_model[]
    };

    let code_model = match code_model_arg {
        "default" => llvm::CodeModelDefault,
        "small" => llvm::CodeModelSmall,
        "kernel" => llvm::CodeModelKernel,
        "medium" => llvm::CodeModelMedium,
        "large" => llvm::CodeModelLarge,
        _ => {
            sess.err(format!("{} is not a valid code model",
                             sess.opts
                                 .cg
                                 .code_model)[]);
            sess.abort_if_errors();
            unreachable!();
        }
    };

    let triple = if sess.targeting_pnacl() {
        // Pretend that we are ARM for name mangling and assembly conventions.
        // https://code.google.com/p/nativeclient/issues/detail?id=2554
        "armv7a-none-nacl-gnueabi"
    } else {
        sess.target.target.llvm_target[]
    };

    let tm = unsafe {
        triple.with_c_str(|t| {
            let cpu = match sess.opts.cg.target_cpu {
                Some(ref s) => s[],
                None => sess.target.target.options.cpu[]
            };
            cpu.with_c_str(|cpu| {
                target_feature(sess).with_c_str(|features| {
                    llvm::LLVMRustCreateTargetMachine(
                        t, cpu, features,
                        code_model,
                        reloc_model,
                        opt_level,
                        true /* EnableSegstk */,
                        use_softfp,
                        no_fp_elim,
                        !any_library && reloc_model == llvm::RelocPIC,
                        ffunction_sections,
                        fdata_sections,
                    )
                })
            })
        })
    };

    if tm.is_null() {
        llvm_err(sess.diagnostic().handler(),
                 format!("Could not create LLVM TargetMachine for triple: {}",
                         triple).to_string());
    } else {
        return tm;
    };
}


/// Module-specific configuration for `optimize_and_codegen`.
#[deriving(Clone)]
pub struct ModuleConfig {
    /// LLVM TargetMachine to use for codegen.
    tm: TargetMachineRef,
    /// Names of additional optimization passes to run.
    passes: Vec<String>,
    /// Some(level) to optimize at a certain level, or None to run
    /// absolutely no optimizations (used for the metadata module).
    opt_level: Option<llvm::CodeGenOptLevel>,

    // Flags indicating which outputs to produce.
    emit_no_opt_bc: bool,
    emit_bc: bool,
    emit_lto_bc: bool,
    emit_ir: bool,
    emit_asm: bool,
    emit_obj: bool,

    // Miscellaneous flags.  These are mostly copied from command-line
    // options.
    no_verify: bool,
    no_prepopulate_passes: bool,
    no_builtins: bool,
    time_passes: bool,
}

impl ModuleConfig {
    pub fn new(tm: TargetMachineRef, passes: Vec<String>) -> ModuleConfig {
        ModuleConfig {
            tm: tm,
            passes: passes,
            opt_level: None,

            emit_no_opt_bc: false,
            emit_bc: false,
            emit_lto_bc: false,
            emit_ir: false,
            emit_asm: false,
            emit_obj: false,

            no_verify: false,
            no_prepopulate_passes: false,
            no_builtins: false,
            time_passes: false,
        }
    }

    pub fn set_flags(&mut self, sess: &Session, trans: &CrateTranslation) {
        self.no_verify = sess.no_verify();
        self.no_prepopulate_passes = sess.opts.cg.no_prepopulate_passes;
        self.no_builtins = trans.no_builtins;
        self.time_passes = sess.time_passes();
    }
}

/// Additional resources used by optimize_and_codegen (not module specific)
struct CodegenContext<'a> {
    // Extra resources used for LTO: (sess, reachable).  This will be `None`
    // when running in a worker thread.
    lto_ctxt: Option<(&'a Session, &'a [String])>,
    // Handler to use for diagnostics produced during codegen.
    handler: &'a Handler,
    // LLVM optimizations for which we want to print remarks.
    remark: Passes,
}

impl<'a> CodegenContext<'a> {
    fn new_with_session(sess: &'a Session, reachable: &'a [String]) -> CodegenContext<'a> {
        CodegenContext {
            lto_ctxt: Some((sess, reachable)),
            handler: sess.diagnostic().handler(),
            remark: sess.opts.cg.remark.clone(),
        }
    }
}

struct HandlerFreeVars<'a> {
    llcx: ContextRef,
    cgcx: &'a CodegenContext<'a>,
}

unsafe extern "C" fn inline_asm_handler(diag: SMDiagnosticRef,
                                        user: *const c_void,
                                        cookie: c_uint) {
    use syntax::codemap::ExpnId;

    let HandlerFreeVars { cgcx, .. }
        = *mem::transmute::<_, *const HandlerFreeVars>(user);

    let msg = llvm::build_string(|s| llvm::LLVMWriteSMDiagnosticToString(diag, s))
        .expect("non-UTF8 SMDiagnostic");

    match cgcx.lto_ctxt {
        Some((sess, _)) => {
            sess.codemap().with_expn_info(ExpnId::from_llvm_cookie(cookie), |info| match info {
                Some(ei) => sess.span_err(ei.call_site, msg[]),
                None     => sess.err(msg[]),
            });
        }

        None => {
            cgcx.handler.err(msg[]);
            cgcx.handler.note("build without -C codegen-units for more exact errors");
        }
    }
}

unsafe extern "C" fn diagnostic_handler(info: DiagnosticInfoRef, user: *mut c_void) {
    let HandlerFreeVars { llcx, cgcx }
        = *mem::transmute::<_, *const HandlerFreeVars>(user);

    match llvm::diagnostic::Diagnostic::unpack(info) {
        llvm::diagnostic::Optimization(opt) => {
            let pass_name = CString::new(opt.pass_name, false);
            let pass_name = pass_name.as_str().expect("got a non-UTF8 pass name from LLVM");
            let enabled = match cgcx.remark {
                AllPasses => true,
                SomePasses(ref v) => v.iter().any(|s| *s == pass_name),
            };

            if enabled {
                let loc = llvm::debug_loc_to_string(llcx, opt.debug_loc);
                cgcx.handler.note(format!("optimization {} for {} at {}: {}",
                                          opt.kind.describe(),
                                          pass_name,
                                          if loc.is_empty() { "[unknown]" } else { loc[] },
                                          llvm::twine_to_string(opt.message))[]);
            }
        }

        _ => (),
    }
}

// Unsafe due to LLVM calls.
unsafe fn optimize_and_codegen(cgcx: &CodegenContext,
                               mtrans: ModuleTranslation,
                               config: ModuleConfig,
                               name_extra: String,
                               output_names: OutputFilenames) {
    let ModuleTranslation { llmod, llcx, .. } = mtrans;
    let tm = config.tm;

    // llcx doesn't outlive this function, so we can put this on the stack.
    let fv = HandlerFreeVars {
        llcx: llcx,
        cgcx: cgcx,
    };
    let fv = &fv as *const HandlerFreeVars as *mut c_void;

    llvm::LLVMSetInlineAsmDiagnosticHandler(llcx, inline_asm_handler, fv);

    if !cgcx.remark.is_empty() {
        llvm::LLVMContextSetDiagnosticHandler(llcx, diagnostic_handler, fv);
    }

    if config.emit_no_opt_bc {
        let ext = format!("{}.no-opt.bc", name_extra);
        output_names.with_extension(ext[]).with_c_str(|buf| {
            llvm::LLVMWriteBitcodeToFile(llmod, buf);
        })
    }

    match config.opt_level {
        Some(opt_level) => {
            // Create the two optimizing pass managers. These mirror what clang
            // does, and are by populated by LLVM's default PassManagerBuilder.
            // Each manager has a different set of passes, but they also share
            // some common passes.
            let fpm = llvm::LLVMCreateFunctionPassManagerForModule(llmod);
            let mpm = llvm::LLVMCreatePassManager();

            // If we're verifying or linting, add them to the function pass
            // manager.
            let addpass = |pass: &str| {
                pass.with_c_str(|s| llvm::LLVMRustAddPass(fpm, s))
            };
            if !config.no_verify { assert!(addpass("verify")); }

            if !config.no_prepopulate_passes {
                llvm::LLVMRustAddAnalysisPasses(tm, fpm, llmod);
                llvm::LLVMRustAddAnalysisPasses(tm, mpm, llmod);
                populate_llvm_passes(fpm, mpm, llmod, opt_level,
                                     config.no_builtins);
            }

            for pass in config.passes.iter() {
                pass.with_c_str(|s| {
                    if !llvm::LLVMRustAddPass(mpm, s) {
                        cgcx.handler.warn(format!("unknown pass {}, ignoring",
                                                  *pass)[]);
                    }
                })
            }

            // Finally, run the actual optimization passes
            time(config.time_passes, "llvm function passes", (), |()|
                 llvm::LLVMRustRunFunctionPassManager(fpm, llmod));
            time(config.time_passes, "llvm module passes", (), |()|
                 llvm::LLVMRunPassManager(mpm, llmod));

            // Deallocate managers that we're now done with
            llvm::LLVMDisposePassManager(fpm);
            llvm::LLVMDisposePassManager(mpm);

            match cgcx.lto_ctxt {
                Some((sess, reachable)) if sess.lto() =>  {
                    time(sess.time_passes(), "all lto passes", (), |()|
                         lto::run(sess, llmod, tm, reachable));

                    if config.emit_lto_bc {
                        let name = format!("{}.lto.bc", name_extra);
                        output_names.with_extension(name[]).with_c_str(|buf| {
                            llvm::LLVMWriteBitcodeToFile(llmod, buf);
                        })
                    }
                },
                _ => {},
            }
        },
        None => {},
    }

    // A codegen-specific pass manager is used to generate object
    // files for an LLVM module.
    //
    // Apparently each of these pass managers is a one-shot kind of
    // thing, so we create a new one for each type of output. The
    // pass manager passed to the closure should be ensured to not
    // escape the closure itself, and the manager should only be
    // used once.
    unsafe fn with_codegen<F>(tm: TargetMachineRef,
                              llmod: ModuleRef,
                              no_builtins: bool,
                              f: F) where
        F: FnOnce(PassManagerRef),
    {
        let cpm = llvm::LLVMCreatePassManager();
        llvm::LLVMRustAddAnalysisPasses(tm, cpm, llmod);
        llvm::LLVMRustAddLibraryInfo(cpm, llmod, no_builtins);
        f(cpm);
        llvm::LLVMDisposePassManager(cpm);
    }

    if config.emit_bc {
        let ext = format!("{}.bc", name_extra);
        output_names.with_extension(ext[]).with_c_str(|buf| {
            llvm::LLVMWriteBitcodeToFile(llmod, buf);
        })
    }

    time(config.time_passes, "codegen passes", (), |()| {
        if config.emit_ir {
            let ext = format!("{}.ll", name_extra);
            output_names.with_extension(ext[]).with_c_str(|output| {
                with_codegen(tm, llmod, config.no_builtins, |cpm| {
                    llvm::LLVMRustPrintModule(cpm, llmod, output);
                })
            })
        }

        if config.emit_asm {
            let path = output_names.with_extension(format!("{}.s", name_extra)[]);
            with_codegen(tm, llmod, config.no_builtins, |cpm| {
                write_output_file(cgcx.handler, tm, cpm, llmod, &path, llvm::AssemblyFileType);
            });
        }

        if config.emit_obj {
            let path = output_names.with_extension(format!("{}.o", name_extra)[]);
            with_codegen(tm, llmod, config.no_builtins, |cpm| {
                write_output_file(cgcx.handler, tm, cpm, llmod, &path, llvm::ObjectFileType);
            });
        }
    });

    llvm::LLVMDisposeModule(llmod);
    llvm::LLVMContextDispose(llcx);
    llvm::LLVMRustDisposeTargetMachine(tm);
}

pub fn run_passes(sess: &Session,
                  trans: &mut CrateTranslation,
                  output_types: &[config::OutputType],
                  crate_output: &OutputFilenames) {
    use llvm::archive_ro::ArchiveRO;
    use std::collections::HashSet;
    use metadata::cstore;
    use libc;
    use back::link::llvm_actual_err;

    // It's possible that we have `codegen_units > 1` but only one item in
    // `trans.modules`.  We could theoretically proceed and do LTO in that
    // case, but it would be confusing to have the validity of
    // `-Z lto -C codegen-units=2` depend on details of the crate being
    // compiled, so we complain regardless.
    // PNaCl uses a bitcode linker, so this isn't an issue.
    if sess.lto() && sess.opts.cg.codegen_units > 1 && !sess.targeting_pnacl() {
        // This case is impossible to handle because LTO expects to be able
        // to combine the entire crate and all its dependencies into a
        // single compilation unit, but each codegen unit is in a separate
        // LLVM context, so they can't easily be combined.
        sess.fatal("can't perform LTO when using multiple codegen units");
    }

    // Sanity check
    assert!(trans.modules.len() == sess.opts.cg.codegen_units);

    unsafe {
        configure_llvm(sess);
    }

    let tm = create_target_machine(sess);

    // Figure out what we actually need to build.

    let mut modules_config = ModuleConfig::new(tm, sess.opts.cg.passes.clone());
    let mut metadata_config = ModuleConfig::new(tm, vec!());

    modules_config.opt_level = Some(get_llvm_opt_level(sess.opts.optimize));

    // Save all versions of the bytecode if we're saving our temporaries.
    if sess.opts.cg.save_temps {
        modules_config.emit_no_opt_bc = true;
        modules_config.emit_bc = true;
        modules_config.emit_lto_bc = true;
        metadata_config.emit_bc = true;
    }

    // Emit bitcode files for the crate if we're emitting an rlib.
    // Whenever an rlib is created, the bitcode is inserted into the
    // archive in order to allow LTO against it.
    let needs_crate_bitcode =
            sess.crate_types.borrow().contains(&config::CrateTypeRlib) &&
            sess.opts.output_types.contains(&config::OutputTypeExe);
    if needs_crate_bitcode || sess.targeting_pnacl() {
        modules_config.emit_bc = true;
    }

    for output_type in output_types.iter() {
        match *output_type {
            config::OutputTypeBitcode => { modules_config.emit_bc = true; },
            config::OutputTypeLlvmAssembly => { modules_config.emit_ir = true; },
            config::OutputTypeAssembly => {
                modules_config.emit_asm = true;
                // If we're not using the LLVM assembler, this function
                // could be invoked specially with output_type_assembly, so
                // in this case we still want the metadata object file.
                if !sess.opts.output_types.contains(&config::OutputTypeAssembly) {
                    metadata_config.emit_obj = true;
                }
            },
            config::OutputTypeObject => { modules_config.emit_obj = true; },
            config::OutputTypeExe => {
                modules_config.emit_obj = true;
                metadata_config.emit_obj = true;
            },
            config::OutputTypeDepInfo => {}
        }
    }

    modules_config.set_flags(sess, trans);
    metadata_config.set_flags(sess, trans);

    if sess.targeting_pnacl() {
        // If targeting PNaCl, never try to run codegen.
        metadata_config.emit_bc = true;
        metadata_config.emit_no_opt_bc = false;
        metadata_config.emit_lto_bc = false;
        metadata_config.emit_ir = false;
        metadata_config.emit_asm = false;
        metadata_config.emit_obj = false;

        modules_config.emit_bc = true;
        modules_config.emit_no_opt_bc = false;
        modules_config.emit_lto_bc = false;
        modules_config.emit_ir = false;
        modules_config.emit_asm = false;
        modules_config.emit_obj = false;
    }


    // Populate a buffer with a list of codegen tasks.  Items are processed in
    // LIFO order, just because it's a tiny bit simpler that way.  (The order
    // doesn't actually matter.)
    let mut work_items = Vec::with_capacity(1 + trans.modules.len());

    {
        let work = build_work_item(sess,
                                   trans.metadata_module.clone(),
                                   metadata_config.clone(),
                                   crate_output.clone(),
                                   "metadata".to_string());
        work_items.push(work);
    }

    for (index, mtrans) in trans.modules.iter().enumerate() {
        let work = build_work_item(sess,
                                   mtrans.clone(),
                                   modules_config.clone(),
                                   crate_output.clone(),
                                   format!("{}", index));
        work_items.push(work);
    }

    fn already_linked_libs(sess: &Session, crates: &Vec<(u32, Option<Path>)>) -> HashSet<String> {
        use metadata::csearch;
        // Go though all extern crates and insert their deps into our linked set.
        let linked = crates
            .iter()
            .fold(HashSet::new(), |mut led: HashSet<String>, &(cnum, _)| {
                let libs = csearch::get_native_libraries(&sess.cstore, cnum);
                for (_, name) in libs.into_iter() {
                    led.insert(name);
                }
                led
            });
        debug!("linked: `{}`", linked);
        linked
    }
    fn pnacl_lib_paths(sess: &Session) -> Vec<Path> {
        use std::os;
        let native_dep_lib_path = Some({
            os::make_absolute(&sess.pnacl_toolchain()
                              .join("le32-nacl")
                              .join("lib"))
                .unwrap()
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
                .unwrap()
        });
        let addl_lib_paths = sess.opts.addl_lib_search_paths.borrow();
        let iter = addl_lib_paths.iter();
        let iter = iter.chain(native_dep_lib_path.iter());
        let iter = iter.chain(ports_lib_path.iter());
        let iter = iter.map(|p| p.clone() );
        iter.collect()
    }

    fn link_attrs_filter(sess: &Session,
                         lib: &String,
                         kind: cstore::NativeLibraryKind,
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

    fn search_for_native(paths: &Vec<Path>,
                         name: &String) -> Option<Path> {
        use std::io::fs::stat;
        let name_path = Path::new(format!("lib{}.a", name));
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

    if sess.targeting_pnacl() {
        let used = sess.cstore.get_used_libraries().borrow();
        let mut linked = already_linked_libs
            (sess, &sess.cstore.get_used_crates(cstore::RequireStatic));
        let lib_paths = pnacl_lib_paths(sess);
        let mut index = 0u;
        let bitcodes: Vec<ModuleTranslation> = (*used)
            .iter()
            .filter_map(|&(ref lib, kind)| {
                link_attrs_filter(sess, lib, kind, &mut linked, &lib_paths)
            })
            .flat_map(|(l, p): (String, Path)| {
                debug!("processing archive `{}`", p.display());
                let archive = ArchiveRO::open(&p)
                    .expect("maybe invalid archive?");
                let mut bitcodes = Vec::new();
                archive.foreach_child(|name, bc| {
                    debug!("processing object `{}`", name);
                    let name_path = Path::new(name);
                    let name_ext_str = name_path.extension_str();
                    if name_ext_str == Some("o") || name_ext_str == Some("obj") {
                        let llctx = unsafe { llvm::LLVMContextCreate() };
                        // Ignore all messages about invalid debug versions (toolchain libraries
                        // cause an abundance of these):
                        unsafe {
                            llvm::LLVMRustSetContextIgnoreDebugMetadataVersionDiagnostics(llctx);
                        }
                        let llmod = name.with_c_str(|s| unsafe {
                            llvm::LLVMRustParseBitcode(llctx,
                                                       s,
                                                       bc.as_ptr() as *const libc::c_void,
                                                       bc.len() as libc::size_t)
                        });
                        unsafe {
                            llvm::LLVMRustResetContextIgnoreDebugMetadataVersionDiagnostics(llctx);
                        }
                        if llmod == ptr::null_mut() {
                            let msg = format!("failed to parse external bitcode
                                              `{}` in archive `{}`",
                                              name, p.display());
                            llvm_actual_err(sess, msg);
                        } else {
                            /*let outs = OutputFilenames {
                                out_directory: crate_output.out_directory.clone(),
                                out_filestem:  name.to_string(),
                                single_output_file: None,
                                extra: "".to_string(),
                            };*/

                            // Some globals in the bitcode from PNaCl have what is
                            // considered invalid linkage in our LLVM (their LLVM is
                            // old). Fortunately, all linkage types get stripped later, so
                            // it's safe to just ignore them all.

                            let name = format!("r-{}-{}-{}",
                                               l, name, index);
                            index = index + 1;

                            let mtrans = ModuleTranslation {
                                llmod: llmod,
                                llcx: llctx,
                                name: Some(name.clone()),
                            };
                            bitcodes.push(mtrans.clone());
                            let work_item = build_work_item(sess,
                                                            mtrans,
                                                            modules_config.clone(),
                                                            crate_output.clone(),
                                                            name);
                            work_items.push(work_item);
                        }
                    }
                });
                bitcodes.into_iter()
            })
            .collect();
        trans.modules.push_all(bitcodes.as_slice());
    }

    // Process the work items, optionally using worker threads.
    if sess.opts.cg.codegen_units == 1 && !sess.targeting_pnacl() {
        run_work_singlethreaded(sess, trans.reachable.as_slice(), work_items);
    } else {
        run_work_multithreaded(sess, work_items, sess.opts.cg.codegen_units);
    }

    // Produce final compile outputs.

    if sess.targeting_pnacl() {
        return;
    }

    let copy_if_one_unit = |ext: &str, output_type: config::OutputType, keep_numbered: bool| {
        // Three cases:
        if sess.opts.cg.codegen_units == 1 {
            // 1) Only one codegen unit.  In this case it's no difficulty
            //    to copy `foo.0.x` to `foo.x`.
            fs::copy(&crate_output.with_extension(ext),
                     &crate_output.path(output_type)).unwrap();
            if !sess.opts.cg.save_temps && !keep_numbered {
                // The user just wants `foo.x`, not `foo.0.x`.
                remove(sess, &crate_output.with_extension(ext));
            }
        } else {
            if crate_output.single_output_file.is_some() {
                // 2) Multiple codegen units, with `-o some_name`.  We have
                //    no good solution for this case, so warn the user.
                sess.warn(format!("ignoring -o because multiple .{} files were produced",
                                  ext)[]);
            } else {
                // 3) Multiple codegen units, but no `-o some_name`.  We
                //    just leave the `foo.0.x` files in place.
                // (We don't have to do any work in this case.)
            }
        }
    };

    let link_obj = |output_path: &Path| {
        // Running `ld -r` on a single input is kind of pointless.
        if sess.opts.cg.codegen_units == 1 {
            fs::copy(&crate_output.with_extension("0.o"),
                     output_path).unwrap();
            // Leave the .0.o file around, to mimic the behavior of the normal
            // code path.
            return;
        }

        // Some builds of MinGW GCC will pass --force-exe-suffix to ld, which
        // will automatically add a .exe extension if the extension is not
        // already .exe or .dll.  To ensure consistent behavior on Windows, we
        // add the .exe suffix explicitly and then rename the output file to
        // the desired path.  This will give the correct behavior whether or
        // not GCC adds --force-exe-suffix.
        let windows_output_path =
            if sess.target.target.options.is_like_windows {
                Some(output_path.with_extension("o.exe"))
            } else {
                None
            };

        let pname = get_cc_prog(sess);
        let mut cmd = Command::new(pname[]);

        cmd.args(sess.target.target.options.pre_link_args[]);
        cmd.arg("-nostdlib");

        for index in range(0, trans.modules.len()) {
            cmd.arg(crate_output.with_extension(format!("{}.o", index)[]));
        }

        cmd.arg("-r")
           .arg("-o")
           .arg(windows_output_path.as_ref().unwrap_or(output_path));

        cmd.args(sess.target.target.options.post_link_args[]);

        if (sess.opts.debugging_opts & config::PRINT_LINK_ARGS) != 0 {
            println!("{}", &cmd);
        }

        cmd.stdin(::std::io::process::Ignored)
           .stdout(::std::io::process::InheritFd(1))
           .stderr(::std::io::process::InheritFd(2));
        match cmd.status() {
            Ok(status) => {
                if !status.success() {
                    sess.err(format!("linking of {} with `{}` failed",
                                     output_path.display(), cmd)[]);
                    sess.abort_if_errors();
                }
            },
            Err(e) => {
                sess.err(format!("could not exec the linker `{}`: {}",
                                 pname,
                                 e)[]);
                sess.abort_if_errors();
            },
        }

        match windows_output_path {
            Some(ref windows_path) => {
                fs::rename(windows_path, output_path).unwrap();
            },
            None => {
                // The file is already named according to `output_path`.
            }
        }
    };

    // Flag to indicate whether the user explicitly requested bitcode.
    // Otherwise, we produced it only as a temporary output, and will need
    // to get rid of it.
    let mut user_wants_bitcode = false;
    for output_type in output_types.iter() {
        match *output_type {
            config::OutputTypeBitcode => {
                user_wants_bitcode = true;
                // Copy to .bc, but always keep the .0.bc.  There is a later
                // check to figure out if we should delete .0.bc files, or keep
                // them for making an rlib.
                copy_if_one_unit("0.bc", config::OutputTypeBitcode, true);
            }
            config::OutputTypeLlvmAssembly => {
                copy_if_one_unit("0.ll", config::OutputTypeLlvmAssembly, false);
            }
            config::OutputTypeAssembly => {
                copy_if_one_unit("0.s", config::OutputTypeAssembly, false);
            }
            config::OutputTypeObject => {
                link_obj(&crate_output.path(config::OutputTypeObject));
            }
            config::OutputTypeExe => {
                // If config::OutputTypeObject is already in the list, then
                // `crate.o` will be handled by the config::OutputTypeObject case.
                // Otherwise, we need to create the temporary object so we
                // can run the linker.
                if !sess.opts.output_types.contains(&config::OutputTypeObject) {
                    link_obj(&crate_output.temp_path(config::OutputTypeObject));
                }
            }
            config::OutputTypeDepInfo => {}
        }
    }
    let user_wants_bitcode = user_wants_bitcode;

    // Clean up unwanted temporary files.

    // We create the following files by default:
    //  - crate.0.bc
    //  - crate.0.o
    //  - crate.metadata.bc
    //  - crate.metadata.o
    //  - crate.o (linked from crate.##.o)
    //  - crate.bc (copied from crate.0.bc)
    // We may create additional files if requested by the user (through
    // `-C save-temps` or `--emit=` flags).

    if !sess.opts.cg.save_temps {
        // Remove the temporary .0.o objects.  If the user didn't
        // explicitly request bitcode (with --emit=bc), and the bitcode is not
        // needed for building an rlib, then we must remove .0.bc as well.

        // Specific rules for keeping .0.bc:
        //  - If we're building an rlib (`needs_crate_bitcode`), then keep
        //    it.
        //  - If the user requested bitcode (`user_wants_bitcode`), and
        //    codegen_units > 1, then keep it.
        //  - If the user requested bitcode but codegen_units == 1, then we
        //    can toss .0.bc because we copied it to .bc earlier.
        //  - If we're not building an rlib and the user didn't request
        //    bitcode, then delete .0.bc.
        // If you change how this works, also update back::link::link_rlib,
        // where .0.bc files are (maybe) deleted after making an rlib.
        let keep_numbered_bitcode = needs_crate_bitcode ||
                (user_wants_bitcode && sess.opts.cg.codegen_units > 1);

        for i in range(0, trans.modules.len()) {
            if modules_config.emit_obj {
                let ext = format!("{}.o", i);
                remove(sess, &crate_output.with_extension(ext[]));
            }

            if modules_config.emit_bc && !keep_numbered_bitcode {
                let ext = format!("{}.bc", i);
                remove(sess, &crate_output.with_extension(ext[]));
            }
        }

        if metadata_config.emit_bc && !user_wants_bitcode {
            remove(sess, &crate_output.with_extension("metadata.bc"));
        }
    }

    // All codegen is finished.
    unsafe {
        llvm::LLVMRustDisposeTargetMachine(tm);
    }

    // We leave the following files around by default:
    //  - crate.o
    //  - crate.metadata.o
    //  - crate.bc
    // These are used in linking steps and will be cleaned up afterward.

    // FIXME: time_llvm_passes support - does this use a global context or
    // something?
    //if sess.time_llvm_passes() { llvm::LLVMRustPrintPassTimings(); }
}

struct WorkItem {
    mtrans: ModuleTranslation,
    config: ModuleConfig,
    output_names: OutputFilenames,
    name_extra: String
}

fn build_work_item(sess: &Session,
                   mtrans: ModuleTranslation,
                   config: ModuleConfig,
                   output_names: OutputFilenames,
                   name_extra: String)
                   -> WorkItem
{
    let mut config = config;
    config.tm = create_target_machine(sess);
    WorkItem { mtrans: mtrans, config: config, output_names: output_names,
               name_extra: name_extra }
}

fn execute_work_item(cgcx: &CodegenContext,
                     work_item: WorkItem) {
    unsafe {
        optimize_and_codegen(cgcx, work_item.mtrans, work_item.config,
                             work_item.name_extra, work_item.output_names);
    }
}

fn run_work_singlethreaded(sess: &Session,
                           reachable: &[String],
                           work_items: Vec<WorkItem>) {
    let cgcx = CodegenContext::new_with_session(sess, reachable);
    let mut work_items = work_items;

    // Since we're running single-threaded, we can pass the session to
    // the proc, allowing `optimize_and_codegen` to perform LTO.
    for work in Unfold::new((), |_| work_items.pop()) {
        execute_work_item(&cgcx, work);
    }
}

fn run_work_multithreaded(sess: &Session,
                          work_items: Vec<WorkItem>,
                          num_workers: uint) {
    // Run some workers to process the work items.
    let work_items_arc = Arc::new(Mutex::new(work_items));
    let mut diag_emitter = SharedEmitter::new();
    let mut futures = Vec::with_capacity(num_workers);

    for i in range(0, num_workers) {
        let work_items_arc = work_items_arc.clone();
        let diag_emitter = diag_emitter.clone();
        let remark = sess.opts.cg.remark.clone();

        let (tx, rx) = channel();
        let mut tx = Some(tx);
        futures.push(rx);

        thread::Builder::new().name(format!("codegen-{}", i)).spawn(move |:| {
            let diag_handler = mk_handler(box diag_emitter);

            // Must construct cgcx inside the proc because it has non-Send
            // fields.
            let cgcx = CodegenContext {
                lto_ctxt: None,
                handler: &diag_handler,
                remark: remark,
            };

            loop {
                // Avoid holding the lock for the entire duration of the match.
                let maybe_work = work_items_arc.lock().pop();
                match maybe_work {
                    Some(work) => {
                        execute_work_item(&cgcx, work);

                        // Make sure to fail the worker so the main thread can
                        // tell that there were errors.
                        cgcx.handler.abort_if_errors();
                    }
                    None => break,
                }
            }

            tx.take().unwrap().send(());
        }).detach();
    }

    let mut panicked = false;
    for rx in futures.into_iter() {
        match rx.recv_opt() {
            Ok(()) => {},
            Err(_) => {
                panicked = true;
            },
        }
        // Display any new diagnostics.
        diag_emitter.dump(sess.diagnostic().handler());
    }
    if panicked {
        sess.fatal("aborting due to worker thread panic");
    }
}

pub fn run_assembler(sess: &Session, outputs: &OutputFilenames) {
    let pname = get_cc_prog(sess);
    let mut cmd = Command::new(pname[]);

    cmd.arg("-c").arg("-o").arg(outputs.path(config::OutputTypeObject))
                           .arg(outputs.temp_path(config::OutputTypeAssembly));
    debug!("{}", &cmd);

    match cmd.output() {
        Ok(prog) => {
            if !prog.status.success() {
                sess.err(format!("linking with `{}` failed: {}",
                                 pname,
                                 prog.status)[]);
                sess.note(format!("{}", &cmd)[]);
                let mut note = prog.error.clone();
                note.push_all(prog.output[]);
                sess.note(str::from_utf8(note[]).unwrap());
                sess.abort_if_errors();
            }
        },
        Err(e) => {
            sess.err(format!("could not exec the linker `{}`: {}",
                             pname,
                             e)[]);
            sess.abort_if_errors();
        }
    }
}

unsafe fn configure_llvm(sess: &Session) {
    use std::sync::{Once, ONCE_INIT};
    static INIT: Once = ONCE_INIT;

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
            llvm_args.push(s.as_ptr());
            llvm_c_strs.push(s);
        };
        add("rustc"); // fake program name
        if vectorize_loop { add("-vectorize-loops"); }
        if vectorize_slp  { add("-vectorize-slp");   }
        if sess.time_llvm_passes() { add("-time-passes"); }
        if sess.print_llvm_passes() { add("-debug-pass=Structure"); }

        for arg in sess.opts.cg.llvm_args.iter() {
            add((*arg)[]);
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

unsafe fn populate_llvm_passes(fpm: llvm::PassManagerRef,
                               mpm: llvm::PassManagerRef,
                               llmod: ModuleRef,
                               opt: llvm::CodeGenOptLevel,
                               no_builtins: bool) {
    // Create the PassManagerBuilder for LLVM. We configure it with
    // reasonable defaults and prepare it to actually populate the pass
    // manager.
    let builder = llvm::LLVMPassManagerBuilderCreate();
    match opt {
        llvm::CodeGenLevelNone => {
            // Don't add lifetime intrinsics at O0
            llvm::LLVMRustAddAlwaysInlinePass(builder, false);
        }
        llvm::CodeGenLevelLess => {
            llvm::LLVMRustAddAlwaysInlinePass(builder, true);
        }
        // numeric values copied from clang
        llvm::CodeGenLevelDefault => {
            llvm::LLVMPassManagerBuilderUseInlinerWithThreshold(builder,
                                                                225);
        }
        llvm::CodeGenLevelAggressive => {
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

    match opt {
        llvm::CodeGenLevelDefault | llvm::CodeGenLevelAggressive => {
            "mergefunc".with_c_str(|s| llvm::LLVMRustAddPass(mpm, s));
        }
        _ => {}
    };
}
