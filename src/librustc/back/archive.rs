// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A helper class for dealing with static archives

use back::link::{get_ar_prog};
use driver::session::Session;
use metadata::filesearch;
use lib::llvm::{ArchiveRef, llvm};

use libc;
use std::io::process::{Command, ProcessOutput};
use std::io::{fs, TempDir};
use std::io;
use std::mem;
use std::os;
use std::raw;
use std::str;
use syntax::abi;

pub static METADATA_FILENAME: &'static str = "rust.metadata.bin";

pub struct Archive<'a> {
    sess: &'a Session,
    ar_prog: Path,
    dst: Path,
    gold_plugin: Option<Path>,
}

pub struct ArchiveRO {
    ptr: ArchiveRef,
}

impl<'a> Archive<'a> {
    /// Initializes a new static archive with the given object file
    pub fn create<'b>(sess: &'a Session, dst: &'b Path,
                      initial_object: &'b Path) -> Archive<'a> {
        let ar = get_ar_prog(sess);
        let archive = Archive {
            sess: sess,
            ar_prog: Path::new(ar),
            dst: dst.clone(),
            gold_plugin: None,
        };
        archive.run_ar("crus", None, [&archive.dst, initial_object]);
        archive
    }

    pub fn new(sess: &'a Session,
               ar: Path,
               dst: Path,
               initial_object: &Path,
               gold_plugin: Option<Path>) -> Archive<'a> {
        let archive = Archive {
            sess: sess,
            ar_prog: ar,
            dst: dst,
            gold_plugin: gold_plugin,
        };
        archive.run_ar("crus", None, [&archive.dst, initial_object]);
        archive
    }

    /// Opens an existing static archive
    pub fn open(sess: &'a Session, dst: Path) -> Archive<'a> {
        assert!(dst.exists());
        let ar = get_ar_prog(sess);
        Archive {
            sess: sess,
            ar_prog: Path::new(ar),
            dst: dst,
            gold_plugin: None,
        }
    }

    /// Adds all of the contents of a native library to this archive. This will
    /// search in the relevant locations for a library named `name`.
    pub fn add_native_library(&mut self, name: &str) -> io::IoResult<()> {
        let location = self.find_library(name);
        self.add_archive(&location, name, [])
    }

    /// Adds all of the contents of the rlib at the specified path to this
    /// archive.
    ///
    /// This ignores adding the bytecode from the rlib, and if LTO is enabled
    /// then the object file also isn't added.
    pub fn add_rlib(&mut self, rlib: &Path, name: &str,
                    lto: bool) -> io::IoResult<()> {
        let object = format!("{}.o", name);
        let bytecode = format!("{}.bytecode.deflate", name);
        let mut ignore = vec!(bytecode.as_slice(), METADATA_FILENAME);
        if lto {
            ignore.push(object.as_slice());
        }
        self.add_archive(rlib, name, ignore.as_slice())
    }

    /// Adds an arbitrary file to this archive
    pub fn add_file(&mut self, file: &Path, has_symbols: bool) {
        let cmd = if has_symbols {"r"} else {"rS"};
        self.run_ar(cmd, None, [&self.dst, file]);
    }

    /// Removes a file from this archive
    pub fn remove_file(&mut self, file: &str) {
        self.run_ar("d", None, [&self.dst, &Path::new(file)]);
    }

    /// Updates all symbols in the archive (runs 'ar s' over it)
    pub fn update_symbols(&mut self) {
        self.run_ar("s", None, [&self.dst]);
    }

    /// Lists all files in an archive
    pub fn files(&self) -> Vec<String> {
        let output = self.run_ar("t", None, [&self.dst]);
        let output = str::from_utf8(output.output.as_slice()).unwrap();
        // use lines_any because windows delimits output with `\r\n` instead of
        // just `\n`
        output.lines_any().map(|s| s.to_string()).collect()
    }

    fn add_archive(&mut self, archive: &Path, name: &str,
                   skip: &[&str]) -> io::IoResult<()> {
        let loc = TempDir::new("rsar").unwrap();

        // First, extract the contents of the archive to a temporary directory
        let archive = os::make_absolute(archive);
        self.run_ar("x", Some(loc.path()), [&archive]);

        // Next, we must rename all of the inputs to "guaranteed unique names".
        // The reason for this is that archives are keyed off the name of the
        // files, so if two files have the same name they will override one
        // another in the archive (bad).
        //
        // We skip any files explicitly desired for skipping, and we also skip
        // all SYMDEF files as these are just magical placeholders which get
        // re-created when we make a new archive anyway.
        let files = try!(fs::readdir(loc.path()));
        let mut inputs = Vec::new();
        for file in files.iter() {
            let filename = file.filename_str().unwrap();
            if skip.iter().any(|s| *s == filename) { continue }
            if filename.contains(".SYMDEF") { continue }

            let filename = format!("r-{}-{}", name, filename);
            // LLDB (as mentioned in back::link) crashes on filenames of exactly
            // 16 bytes in length. If we're including an object file with
            // exactly 16-bytes of characters, give it some prefix so that it's
            // not 16 bytes.
            let filename = if filename.len() == 16 {
                format!("lldb-fix-{}", filename)
            } else {
                filename
            };
            let new_filename = file.with_filename(filename);
            try!(fs::rename(file, &new_filename));
            inputs.push(new_filename);
        }
        if inputs.len() == 0 { return Ok(()) }

        // Finally, add all the renamed files to this archive
        let mut args = vec!(&self.dst);
        args.extend(inputs.iter());
        self.run_ar("r", None, args.as_slice());
        Ok(())
    }

    fn find_library(&self, name: &str) -> Path {
        let (osprefix, osext) = match self.sess.targ_cfg.os {
            abi::OsWin32 => ("", "lib"), _ => ("lib", "a"),
        };
        // On Windows, static libraries sometimes show up as libfoo.a and other
        // times show up as foo.lib
        let oslibname = format!("{}{}.{}", osprefix, name, osext);
        let unixlibname = format!("lib{}.a", name);

        let mut rustpath = filesearch::rust_path();
        rustpath.push(self.sess.target_filesearch().get_lib_path());
        let search = self.sess.opts.addl_lib_search_paths.borrow();
        for path in search.iter().chain(rustpath.iter()) {
            debug!("looking for {} inside {}", name, path.display());
            let test = path.join(oslibname.as_slice());
            if test.exists() { return test }
            if oslibname != unixlibname {
                let test = path.join(unixlibname.as_slice());
                if test.exists() { return test }
            }
        }
        self.sess.fatal(format!("could not find native static library `{}`, \
                                 perhaps an -L flag is missing?",
                                name).as_slice());
    }

    fn run_ar(&self, args: &str, cwd: Option<&Path>,
              paths: &[&Path]) -> ProcessOutput {
        let mut cmd = Command::new(&self.ar_prog);

        cmd.arg(args);

        match self.gold_plugin {
            Some(ref path) => {
                cmd.args(&[format!("--plugin={}", path.display())]);
            }
            None => {}
        };

        cmd.args(paths);
        debug!("{}", cmd);

        match cwd {
            Some(p) => {
                cmd.cwd(p);
                debug!("inside {}", p.display());
            }
            None => {}
        }

        match cmd.spawn() {
            Ok(prog) => {
                let o = prog.wait_with_output().unwrap();
                if !o.status.success() {
                    self.sess.err(format!("{} failed with: {}",
                                     cmd,
                                     o.status).as_slice());
                    self.sess.note(format!("stdout ---\n{}",
                                      str::from_utf8(o.output
                                                     .as_slice()).unwrap())
                              .as_slice());
                    self.sess.note(format!("stderr ---\n{}",
                                      str::from_utf8(o.error
                                                     .as_slice()).unwrap())
                              .as_slice());
                    self.sess.abort_if_errors();
                }
                o
            },
            Err(e) => {
                self.sess.err(format!("could not exec `{}`: {}", self.ar_prog.display(),
                                 e).as_slice());
                self.sess.abort_if_errors();
                fail!("rustc::back::archive::run_ar() should not reach this point");
            }
        }
    }
}

impl ArchiveRO {
    /// Opens a static archive for read-only purposes. This is more optimized
    /// than the `open` method because it uses LLVM's internal `Archive` class
    /// rather than shelling out to `ar` for everything.
    ///
    /// If this archive is used with a mutable method, then an error will be
    /// raised.
    pub fn open(dst: &Path) -> Option<ArchiveRO> {
        unsafe {
            let ar = dst.with_c_str(|dst| {
                llvm::LLVMRustOpenArchive(dst)
            });
            if ar.is_null() {
                None
            } else {
                Some(ArchiveRO { ptr: ar })
            }
        }
    }

    /// Reads a file in the archive
    pub fn read<'a>(&'a self, file: &str) -> Option<&'a [u8]> {
        unsafe {
            let mut size = 0 as libc::size_t;
            let ptr = file.with_c_str(|file| {
                llvm::LLVMRustArchiveReadSection(self.ptr, file, &mut size)
            });
            if ptr.is_null() {
                None
            } else {
                Some(mem::transmute(raw::Slice {
                    data: ptr,
                    len: size as uint,
                }))
            }
        }
    }
    // Reads every child, running f on each.
    pub fn foreach_child(&self, f: |&str, &[u8]|) {
        use std::mem::transmute;
        extern "C" fn cb(name: *const libc::c_uchar,   name_len: libc::size_t,
                         buffer: *const libc::c_uchar, buffer_len: libc::size_t,
                         f: *mut libc::c_void) {
            use std::str::from_utf8_lossy;
            use std::slice::raw::buf_as_slice;
            use std::mem::transmute_copy;
            let f: &|&str, &[u8]| = unsafe { transmute(f) };
            unsafe {
                buf_as_slice(name as *const u8, name_len as uint, |name_buf| {
                    let name = from_utf8_lossy(name_buf).into_owned();
                    debug!("running f on `{}`", name);
                    buf_as_slice(buffer, buffer_len as uint, |buf| {
                        let f: |&str, &[u8]| = transmute_copy(f);
                        f(name.as_slice(), buf);
                    })
                })
            }
        }
        unsafe {
            llvm::LLVMRustArchiveReadAllChildren(self.ptr,
                                                 cb,
                                                 transmute(&f));
        }
    }
}

impl Drop for ArchiveRO {
    fn drop(&mut self) {
        unsafe {
            llvm::LLVMRustDestroyArchive(self.ptr);
        }
    }
}
