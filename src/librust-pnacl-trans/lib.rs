// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![crate_name = "rust-pnacl-trans"]
#![experimental]
#![comment = "The Rust translator for PNaCl targets"]
#![license = "MIT/ASL2"]
#![crate_type = "dylib"]
#![crate_type = "rlib"]
#![doc(html_logo_url = "http://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "http://www.rust-lang.org/favicon.ico",
       html_root_url = "http://doc.rust-lang.org/master/")]

#![feature(phase)]

extern crate getopts;
extern crate libc;
#[phase(plugin, link)]
extern crate log;
extern crate "rustc_llvm" as llvm;

use getopts::{optopt, optflag, getopts, reqopt, optmulti, OptGroup, Matches};
use std::c_str::CString;
use std::collections::{HashSet, HashMap};
use std::fmt::Show;
use std::io::fs::File;
use std::io::process::{Command, InheritFd, ExitStatus};
use std::os;
use std::ptr;

// From librustc:
pub fn host_triple() -> &'static str {
    // Get the host triple out of the build environment. This ensures that our
    // idea of the host triple is the same as for the set of libraries we've
    // actually built.  We can't just take LLVM's host triple because they
    // normalize all ix86 architectures to i386.
    //
    // Instead of grabbing the host triple (for the current host), we grab (at
    // compile time) the target triple that this rustc is built with and
    // calling that (at runtime) the host triple.
    (option_env!("CFG_COMPILER_HOST_TRIPLE")).
        expect("CFG_COMPILER_HOST_TRIPLE")
}

fn get_native_arch() -> &'static str {
    use std::os::consts::ARCH;
    match ARCH {
        "x86" => "i686",
        _ => ARCH,
    }
}

fn optgroups() -> Vec<OptGroup> {
    vec!(optflag("h", "help", "Display this message"),
         reqopt("o", "", "Write the linked output to this file", ""),
         optopt("", "opt-level", "Optimize with possible levels 0-3", "LEVEL"),
         optflag("", "save-temps", "Save temp files"),
         optopt("", "target", "The target triple to codegen for", ""),
         reqopt("", "cross-path", "The path to the Pepper SDK", ""),
         optmulti("", "raw", "The specified bitcodes have had none of the usual PNaCl IR \
                              legalization passes run on them", ""),
         optflag("", "all-raw", "All input bitcodes are of raw form"))
}
fn fatal<T: Str + Show>(msg: T) -> ! {
    println!("error: {}", msg);
    os::set_exit_status(1);
    fail!("fatal error");
}

fn warn<T: Str + Show>(msg: T) {
    println!("warning: {}", msg);
}

pub fn llvm_warn<T: Str + Show>(msg: T) {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            warn(msg);
        } else {
            let err = CString::new(cstr, true);
            let err = String::from_utf8_lossy(err.as_bytes());
            warn(format!("{}: {}",
                         msg.as_slice(),
                         err.as_slice()));
        }
    }
}

pub fn main() {
    let args: Vec<String> = os::args();
    let opts = optgroups();

    let matches = match getopts(args.tail(), opts.as_slice()) {
        Ok(m) => m,
        Err(f) => fail!(f.to_string()),
    };
    if matches.opt_present("h") {
        println!("{}", getopts::usage("pexe/bc -> nexe translator and linker", opts.as_slice()));
        return;
    }

    let sysroot = match os::self_exe_path().map(|p| p.join("..") ) {
        Some(sysroot) => sysroot,
        None => fatal("I need the sysroot"),
    };

    let opt_level = {
        let opt_level_str = match matches.opt_str("opt-level") {
            Some(level) => level,
            None => "0".to_string(),
        };
        match opt_level_str.as_slice() {
            "0" => llvm::CodeGenLevelNone,
            "1" => llvm::CodeGenLevelLess,
            "2" => llvm::CodeGenLevelDefault,
            "3" => llvm::CodeGenLevelAggressive,
            lvl => {
                fatal(format!("invalid optimization level: `{}`", lvl));
            }
        }
    };
    let triple = match matches.opt_str("target") {
        Some(target) => {
            if !target.as_slice().contains("nacl") {
                fatal("invalid non-NaCl triple");
            }
            target
        }
        None => {
            format!("{}-none-nacl-gnu", get_native_arch())
        }
    };
    let cross_path = matches.opt_str("cross-path").unwrap();
    let cross_path = os::make_absolute(&Path::new(cross_path));
    let all_raw = matches.opt_present("all-raw");
    let mut input: Vec<(String, bool)> = matches.free
            .iter()
            .map(|i| (i.clone(), all_raw) )
            .collect();

    let output = matches.opt_str("o").unwrap();
    let ctxt = unsafe { llvm::LLVMContextCreate() };

    unsafe {
        llvm::LLVMInitializePasses();
    }

    {
        let raw_bitcodes_vec = matches.opt_strs("raw");
        let mut raw_bitcodes = HashSet::new();
        for i in raw_bitcodes_vec.move_iter() {
            if !raw_bitcodes.insert(i.clone()) {
                warn(format!("file specified two or more times in --raw: `{}`", i));
            }
            input.push((i, true));
        }
    }

    if input.is_empty() {
        fatal("missing input file(s)");
    }

    let mut bc_input = HashMap::new();
    let mut obj_input: Vec<String> = Vec::new();
    for (i, is_raw) in input.move_iter() {
        let bc = match File::open(&Path::new(i.clone())).read_to_end() {
            Ok(buf) => buf,
            Err(e) => {
                warn(format!("error reading file `{}`: `{}`",
                             i, e));
                continue;
            }
        };
        let llmod = i.with_c_str(|s| {
            unsafe {
                llvm::LLVMRustParseBitcode(ctxt,
                                           s,
                                           bc.as_ptr() as *const libc::c_void,
                                           bc.len() as libc::size_t)
            }
        });
        if llmod == ptr::mut_null() {
            if is_raw {
                warn(format!("raw bitcode isn't bitcode: `{}`", i));
            }
            obj_input.push(i);
        } else {
            if is_raw {
                unsafe {
                    let pm = llvm::LLVMCreatePassManager();
                    "pnacl-sjlj-eh".with_c_str(|s| assert!(llvm::LLVMRustAddPass(pm, s)) );
                    "expand-varargs".with_c_str(|s| assert!(llvm::LLVMRustAddPass(pm, s)) );
                    llvm::LLVMRunPassManager(pm, llmod);
                    llvm::LLVMDisposePassManager(pm);
                }
            }
            if !bc_input.insert(i.clone(), llmod) {
                warn(format!("file specified two or more times: `{}`", i));
            }
        }
    }

    unsafe {
        let mut llvm_c_strs = Vec::new();
        let mut llvm_args = Vec::new();
        {
            let add = |arg: &str| {
                let s = arg.to_c_str();
                llvm_args.push(s.as_ptr());
                llvm_c_strs.push(s);
                debug!("adding llvm arg: `{}`", arg);
            };
            add("rust-pnacl-trans");
            if !(triple.as_slice().contains("i386") ||
                 triple.as_slice().contains("i486") ||
                 triple.as_slice().contains("i586") ||
                 triple.as_slice().contains("i686") ||
                 triple.as_slice().contains("i786")) {
                add("-mtls-use-call");
            }
        }

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

        llvm::LLVMRustSetLLVMOptions(llvm_args.len() as libc::c_int,
                                     llvm_args.as_ptr());
    }
    let tm = unsafe {
        triple.with_c_str(|t| {
            "generic".with_c_str(|cpu| {
                "".with_c_str(|features| {
                    llvm::LLVMRustCreateTargetMachine(
                        t, cpu, features,
                        llvm::CodeModelSmall,
                        llvm::RelocDefault,
                        opt_level,
                        false /* EnableSegstk */,
                        false /* soft fp */,
                        false /* frame elim */,
                        true  /* ffunction_sections */,
                        true  /* fdata_sections */)
                })
            })
        })
    };

    let obj_input: Vec<String> = bc_input
        .move_iter()
        .filter_map(|(i, llmod)| {
            debug!("translating `{}`", i);
            unsafe {
                let pm = llvm::LLVMCreatePassManager();

                "add-pnacl-external-decls".with_c_str(|s| assert!(llvm::LLVMRustAddPass(pm, s)) );
                "resolve-pnacl-intrinsics".with_c_str(|s| assert!(llvm::LLVMRustAddPass(pm, s)) );

                llvm::LLVMRustAddAnalysisPasses(tm, pm, llmod);
                llvm::LLVMRustAddLibraryInfo(pm, llmod, false);

                "combine-vector-instructions".with_c_str(|s| {
                    assert!(llvm::LLVMRustAddPass(pm, s))
                });

                let out = format!("{}.o", i);

                let success = out.with_c_str(|o| {
                    llvm::LLVMRustWriteOutputFile(tm,
                                                  pm,
                                                  llmod,
                                                  o,
                                                  llvm::ObjectFile)
                });

                llvm::LLVMDisposePassManager(pm);

                if success {
                    Some(out)
                } else {
                    llvm_warn("error writing output");
                    None
                }
            }
        })
        .chain(obj_input.move_iter())
        .collect();

    unsafe {
        llvm::LLVMRustDisposeTargetMachine(tm);
    }

    let arch = if triple.as_slice().contains("x86_64") {
        "x86-64"
    } else if triple.as_slice().contains("i386") ||
              triple.as_slice().contains("i486") ||
              triple.as_slice().contains("i586") ||
              triple.as_slice().contains("i686") ||
              triple.as_slice().contains("i786") {
        "x86-32"
    } else if triple.as_slice().contains("arm") {
        "arm"
    } else if triple.as_slice().contains("mips") {
        "mips"
    } else {
        unreachable!()
    };

    debug!("linking");
    let lib_path = cross_path
        .join_many(["toolchain".to_string(),
                    {
                        let mut s = pnacl_toolchain_prefix();
                        s.push_str("_pnacl");
                        s
                    },
                    format!("lib-{}", arch)]);

    let nexe_link_args = vec!("-nostdlib".to_string(),
                              "--no-fix-cortex-a8".to_string(),
                              "--eh-frame-hdr".to_string(),
                              "-z".to_string(), "text".to_string(),
                              "--build-id".to_string(),
                              "--entry=__pnacl_start".to_string(),
                              "-static".to_string(),
                              lib_path.join("crtbegin.o")
                                  .display().as_maybe_owned().to_string());
    let nexe_link_args_suffix = vec!(lib_path.join("libpnacl_irt_shim.a")
                                         .display().as_maybe_owned().to_string(),
                                     "--start-group".to_string(),
                                     lib_path.join("libgcc.a")
                                         .display().as_maybe_owned().to_string(),
                                     lib_path.join("libcrt_platform.a")
                                         .display().as_maybe_owned().to_string(),
                                     "--end-group".to_string(),
                                     lib_path.join("crtend.o")
                                         .display().as_maybe_owned().to_string(),
                                     "--undefined=_start".to_string(),
                                     "-o".to_string(),
                                     output.clone());
    let nexe_link_args: Vec<String> = nexe_link_args.move_iter()
        .chain(obj_input.iter().map(|o| o.clone() ))
        .chain(nexe_link_args_suffix.move_iter())
        .collect();

    let gold = sysroot.join_many(["lib".to_string(),
                                  "rustlib".to_string(),
                                  host_triple().to_string(),
                                  "bin".to_string(),
                                  "le32-nacl-ld.gold".to_string()]);

    fn cleanup_objs(m: &Matches, objs: Vec<String>) {
        use std::io::fs::unlink;

        if m.opt_present("save-temps") {
            return;
        }

        for i in objs.move_iter() {
            debug!("cleaning up `{}`", i);
            let _ = unlink(&Path::new(i));
        }
    }

    let mut cmd = Command::new(gold);
    cmd.args(nexe_link_args.as_slice());
    cmd.stdout(InheritFd(libc::STDOUT_FILENO));
    cmd.stderr(InheritFd(libc::STDERR_FILENO));
    debug!("running linker: `{}`", cmd);
    match cmd.spawn() {
        Ok(mut process) => {
            match process.wait() {
                Ok(ExitStatus(status)) => {
                    os::set_exit_status(status);
                }
                Ok(_) => {
                    os::set_exit_status(1);
                }
                Err(e) => {
                    cleanup_objs(&matches, obj_input);
                    fatal(format!("couldn't wait on the linker: {}", e));
                }
            }
        }
        Err(e) => {
            cleanup_objs(&matches, obj_input);
            fatal(format!("couldn't spawn linker: {}", e));
        }
    }
    cleanup_objs(&matches, obj_input);
}

#[cfg(target_os = "linux")]
fn pnacl_toolchain_prefix() -> String {
    "linux".to_string()
}
