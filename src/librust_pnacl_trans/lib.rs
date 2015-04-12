// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![crate_name = "rust_pnacl_trans"]
#![crate_type = "dylib"]
#![crate_type = "rlib"]
#![doc(html_logo_url = "http://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "http://www.rust-lang.org/favicon.ico",
       html_root_url = "http://doc.rust-lang.org/master/")]
#![cfg_attr(target_os = "nacl", allow(dead_code))]

#![feature(rustc_private, libc, collections, exit_status)]

extern crate getopts;
extern crate libc;
extern crate rustc_llvm as llvm;

#[macro_use]
extern crate log;

use getopts::{optopt, optflag, getopts, reqopt, optmulti, OptGroup, Matches};
use std::collections::{HashSet, HashMap};
use std::fs::File;
use std::process::{Command, Stdio};
use std::env;
use std::ptr;
use std::ffi;
use std::fmt::Display;
use std::path::{Path, PathBuf};

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
    use std::env::consts::ARCH;
    match ARCH {
        "x86" => "i686",
        _ => ARCH,
    }
}

fn inner_optgroups() -> Vec<OptGroup> {
    vec!(optflag("h", "help", "Display this message"),
         reqopt("o", "", "Write the linked output to this file", ""),
         optopt("", "opt-level", "Optimize with possible levels 0-3", "LEVEL"),
         optflag("", "save-temps", "Save temp files"),
         optopt("", "target", "The target triple to codegen for", ""),
         optmulti("", "raw", "The specified bitcodes have had none of the usual PNaCl IR \
                              legalization passes run on them", ""),
         optflag("", "all-raw", "All input bitcodes are of raw form"))
}
#[cfg(not(target_os = "nacl"))]
fn optgroups() -> Vec<OptGroup> {
    let mut opts = inner_optgroups();
    opts.push(reqopt("", "cross-path", "The path to the Pepper SDK", ""));
    opts
}
#[cfg(target_os = "nacl")]
fn optgroups() -> Vec<OptGroup> {
    let mut opts = inner_optgroups();
    // on nacl
    opts.push(optopt("", "cross-path", "Ignored", ""));
    opts
}

fn fatal<T: Display>(msg: T) -> ! {
    println!("error: {}", msg);
    env::set_exit_status(1);
    panic!("fatal error");
}

fn warn<T: Display>(msg: T) {
    println!("warning: {}", msg);
}

pub fn llvm_warn<T: Display>(msg: T) {
    unsafe {
        let cstr = llvm::LLVMRustGetLastError();
        if cstr == ptr::null() {
            warn(msg);
        } else {
            let err = ffi::CStr::from_ptr(cstr).to_bytes();
            let err = String::from_utf8_lossy(&err[..]).to_string();
            warn(format!("{}: {}",
                         msg, err));
        }
    }
}

pub fn main() {
    let args: Vec<String> = env::args().collect();

    let is_host_nacl = host_triple().ends_with("nacl");

    let opts = optgroups();

    let matches = match getopts(args.tail(), &opts[..]) {
        Ok(m) => m,
        Err(f) => panic!(f.to_string()),
    };
    if matches.opt_present("h") {
        println!("{}", getopts::usage("pexe/bc -> nexe translator and linker", &opts[..]));
        return;
    }

    let opt_level = {
        let opt_level_str = match matches.opt_str("opt-level") {
            Some(level) => level,
            None => "0".to_string(),
        };
        match &opt_level_str[..] {
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
            if !&target[..].contains("nacl") {
                fatal("invalid non-NaCl triple");
            }
            target
        }
        None => {
            format!("{}-none-nacl-gnu", get_native_arch())
        }
    };

    let cross_path = if !is_host_nacl {
        let cross_path = matches.opt_str("cross-path").unwrap();
        env::current_dir()
            .unwrap()
            .join(&cross_path[..])
    } else {
        Path::new("/")
            .to_path_buf()
    };

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
        for i in raw_bitcodes_vec.into_iter() {
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
    for (i, is_raw) in input.into_iter() {
        let bc = match File::open(&Path::new(&i))
            .and_then(|mut f| {
                use std::io::Read;
                let mut b = Vec::new();
                try!(f.read_to_end(&mut b));
                Ok(b)
            })
        {
            Ok(buf) => buf,
            Err(e) => {
                warn(format!("error reading file `{:?}`: `{}`",
                             i, e));
                continue;
            }
        };
        let llmod = unsafe {
            llvm::LLVMRustParseBitcode(ctxt,
                                       i.as_ptr() as *const i8,
                                       bc.as_ptr() as *const libc::c_void,
                                       bc.len() as libc::size_t)
        };
        if llmod == ptr::null_mut() {
            if is_raw {
                warn(format!("raw bitcode isn't bitcode: `{:?}`", i));
            }
            obj_input.push(i);
        } else {
            if is_raw {
                unsafe {
                    let pm = llvm::LLVMCreatePassManager();
                    assert!(llvm::LLVMRustAddPass(pm, "pnacl-sjlj-eh\0".as_ptr() as *const i8 ));
                    assert!(llvm::LLVMRustAddPass(pm, "expand-varargs\0".as_ptr() as *const i8 ));
                    llvm::LLVMRunPassManager(pm, llmod);
                    llvm::LLVMDisposePassManager(pm);
                }
            }
            if bc_input.insert(i.clone(), llmod).is_some() {
                warn(format!("file specified two or more times: `{:?}`", i));
            }
        }
    }

    unsafe {
        let mut llvm_args = Vec::new();
        {
            let mut add = |arg: &str| {
                llvm_args.push(arg.as_ptr() as *const i8);
                debug!("adding llvm arg: `{}`", arg);
            };
            add("rust_pnacl_trans\0");
            if !(triple.contains("i386") ||
                 triple.contains("i486") ||
                 triple.contains("i586") ||
                 triple.contains("i686") ||
                 triple.contains("i786")) {
                add("-mtls-use-call\0");
            }
        }

        // Only initialize the platforms supported by PNaCl && Rust here,
        // because using --llvm-root will have multiple platforms that rustllvm
        // doesn't actually link to and it's pointless to put target info into
        // the registry that Rust cannot generate machine code for.
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
        let triple = format!("{}\0", triple);
        let triple_ptr = triple.as_ptr() as *const i8;
        let cpu_ptr = "generic\0".as_ptr() as *const i8;
        let features_ptr = "\0".as_ptr() as *const i8;
        llvm::LLVMRustCreateTargetMachine(triple_ptr,
                                          cpu_ptr,
                                          features_ptr,
                                          llvm::CodeModelSmall,
                                          llvm::RelocDefault,
                                          opt_level,
                                          false /* EnableSegstk */,
                                          false /* soft fp */,
                                          false /* frame elim */,
                                          true  /* pie */,
                                          true  /* ffunction_sections */,
                                          true  /* fdata_sections */)
    };

    let obj_input: Vec<String> = bc_input
        .into_iter()
        .filter_map(|(i, llmod)| {
            debug!("translating `{}`", i);
            unsafe {
                let pm = llvm::LLVMCreatePassManager();

                assert!(llvm::LLVMRustAddPass(pm,
                                              "add-pnacl-external-decls\0"
                                                   .as_ptr() as *const i8));
                assert!(llvm::LLVMRustAddPass(pm,
                                              "resolve-pnacl-intrinsics\0"
                                                   .as_ptr() as *const i8));

                llvm::LLVMRustAddAnalysisPasses(tm, pm, llmod);
                llvm::LLVMRustAddLibraryInfo(pm, llmod, false);

                assert!(llvm::LLVMRustAddPass(pm,
                                              "backend-canonicalize\0"
                                                   .as_ptr() as *const i8));

                let out = format!("{}.o", i);
                let out_cstr = format!("{}\0", out);

                let success = llvm::LLVMRustWriteOutputFile(tm,
                                                            pm,
                                                            llmod,
                                                            out_cstr.as_ptr() as *const i8,
                                                            llvm::ObjectFileType);

                llvm::LLVMDisposePassManager(pm);

                if success {
                    Some(out)
                } else {
                    llvm_warn("error writing output");
                    None
                }
            }
        })
        .chain(obj_input.into_iter())
        .collect();

    unsafe {
        llvm::LLVMRustDisposeTargetMachine(tm);
    }

    let arch = if triple.contains("x86_64") {
        "x86-64"
    } else if triple.contains("i386") ||
              triple.contains("i486") ||
              triple.contains("i586") ||
              triple.contains("i686") ||
              triple.contains("i786") {
        "x86-32"
    } else if triple.contains("arm") {
        "arm"
    } else if triple.contains("mips") {
        "mips"
    } else {
        unreachable!()
    };

    debug!("linking");

    static NATIVE_SUPPORT_LIBS: &'static [&'static str] =
        &["crtbegin.o",
          "libpnacl_irt_shim.a",
          "libgcc.a",
          "libcrt_platform.a",
          "crtend.o"];

    #[cfg(not(target_os = "nacl"))]
    fn get_native_support_lib_paths<P: AsRef<Path>>(cross_path: P, arch: &str)
        -> Vec<String>
    {
        let mut toolchain_path = cross_path
            .as_ref()
            .join("toolchain")
            .to_path_buf();
        toolchain_path.push(&{
            let mut s = pnacl_toolchain_prefix();
            s.push_str("_pnacl");
            s
        });


        let mut tcp = toolchain_path;
        tcp.push("translator");
        tcp.push(arch);
        tcp.push("lib");

        NATIVE_SUPPORT_LIBS.iter()
            .map(|lib| {
                tcp.clone()
                    .join(lib)
                    .into_os_string()
                    .into_string()
                    .unwrap()
            })
            .collect()
    }
    #[cfg(target_os = "nacl")]
    fn get_native_support_lib_paths<P: AsRef<Path>>(_: P, arch: &str) -> Vec<String> {
        let lib_path = Path::new("/lib")
            .join("translator")
            .join(arch);
        NATIVE_SUPPORT_LIBS.iter()
            .map(|lib| {
                lib_path
                    .join(lib)
                    .into_os_string()
                    .into_string()
                    .unwrap()
            })
            .collect()
    }

    let libs = get_native_support_lib_paths(&cross_path, arch);
    debug_assert!(libs.len() == 5);
    let crtbegin = libs[0].clone();
    let pnacl_irt_shim = libs[1].clone();
    let gcc = libs[2].clone();
    let crt_platform = libs[3].clone();
    let crtend = libs[4].clone();

    let nexe_link_args = vec!("-nostdlib".to_string(),
                              "--no-fix-cortex-a8".to_string(),
                              "--eh-frame-hdr".to_string(),
                              "-z".to_string(), "text".to_string(),
                              "--build-id".to_string(),
                              "--entry=__pnacl_start".to_string(),
                              "-static".to_string(),
                              crtbegin);
    let nexe_link_args_suffix = vec!(pnacl_irt_shim,
                                     "--start-group".to_string(),
                                     gcc,
                                     crt_platform,
                                     "--end-group".to_string(),
                                     crtend,
                                     "--undefined=_start".to_string(),
                                     "-o".to_string(),
                                     output.clone());

    let nexe_link_args: Vec<String> = nexe_link_args.into_iter()
        .chain(obj_input.iter().map(|o| o.clone() ))
        .chain(nexe_link_args_suffix.into_iter())
        .collect();

    #[cfg(not(target_os = "nacl"))]
    fn get_linker<P: AsRef<Path>>(cross_path: P) -> PathBuf {
        let mut toolchain_path = cross_path
            .as_ref()
            .to_path_buf();
        toolchain_path.push("toolchain");
        toolchain_path.push(&{
            let mut s = pnacl_toolchain_prefix();
            s.push_str("_pnacl");
            s
        });

        let mut gold = toolchain_path;
        gold.push("bin");
        gold.push("le32-nacl-ld.gold");
        gold
    }
    #[cfg(target_os = "nacl")]
    fn get_linker<P: AsRef<Path>>(_: Path) -> PathBuf {
        use std::env::consts::EXE_SUFFIX;
        let linker = format!("ld.gold{}",
                             EXE_SUFFIX);
        Path::new("/bin")
            .join(&linker)
            .to_path_buf()
    }

    let gold = get_linker(&cross_path);

    fn cleanup_objs(m: &Matches, objs: Vec<String>) {
        use std::fs::remove_file;

        if m.opt_present("save-temps") {
            return;
        }

        for i in objs.into_iter() {
            let r = remove_file(&i);
            debug!("cleaning up `{}`: `{:?}`", i, r);
        }
    }

    let mut cmd = Command::new(&gold);
    cmd.args(&nexe_link_args[..]);
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    debug!("running linker: `{:?}`", cmd);
    match cmd.spawn() {
        Ok(mut process) => {
            match process.wait() {
                Ok(status) => {
                    env::set_exit_status(status.code().unwrap_or(1));
                },
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
#[cfg(target_os = "macos")]
fn pnacl_toolchain_prefix() -> String {
    "mac".to_string()
}
#[cfg(windows)]
fn pnacl_toolchain_prefix() -> String {
    "win".to_string()
}
#[cfg(all(not(windows),
          not(target_os = "linux"),
          not(target_os = "macos")))]
fn pnacl_toolchain_prefix() -> String {
    unimplemented!();
}
