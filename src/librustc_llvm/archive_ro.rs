// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A wrapper around LLVM's archive (.a) code

use libc;
use ArchiveRef;

use std::ffi::CString;
use std::slice;
use std::path::Path;

pub struct ArchiveRO {
    ptr: ArchiveRef,
}

impl ArchiveRO {
    /// Opens a static archive for read-only purposes. This is more optimized
    /// than the `open` method because it uses LLVM's internal `Archive` class
    /// rather than shelling out to `ar` for everything.
    ///
    /// If this archive is used with a mutable method, then an error will be
    /// raised.
    pub fn open(dst: &Path) -> Option<ArchiveRO> {
        return unsafe {
            let s = path2cstr(dst);
            let ar = ::LLVMRustOpenArchive(s.as_ptr());
            if ar.is_null() {
                None
            } else {
                Some(ArchiveRO { ptr: ar })
            }
        };

        #[cfg(unix)]
        fn path2cstr(p: &Path) -> CString {
            use std::os::unix::prelude::*;
            use std::ffi::AsOsStr;
            CString::new(p.as_os_str().as_bytes()).unwrap()
        }
        #[cfg(windows)]
        fn path2cstr(p: &Path) -> CString {
            CString::new(p.to_str().unwrap()).unwrap()
        }
    }

    /// Reads a file in the archive
    pub fn read<'a>(&'a self, file: &str) -> Option<&'a [u8]> {
        unsafe {
            let mut size = 0 as libc::size_t;
            let file = CString::new(file).unwrap();
            let ptr = ::LLVMRustArchiveReadSection(self.ptr, file.as_ptr(),
                                                   &mut size);
            if ptr.is_null() {
                None
            } else {
                Some(slice::from_raw_parts(ptr as *const u8, size as uint))
            }
        }
    }

        // Reads every child, running f on each.
    pub fn foreach_child<F>(&self, f: F) where F : FnMut(&str, &[u8]), {
        use std::mem::transmute;
        extern "C" fn cb<F>(name: *const libc::c_uchar,   name_len: libc::size_t,
                            buffer: *const libc::c_uchar, buffer_len: libc::size_t,
                            f: *mut libc::c_void) where F : FnMut(&str, &[u8]), {
            use std::slice::from_raw_parts;
            let f: &mut F = unsafe { transmute(f) };
            let name = name as *const u8;
            unsafe {
                let name_buf = from_raw_parts(name, name_len as uint);
                let name = String::from_utf8_lossy(name_buf);
                debug!("running f on `{}`", name);
                let buf = from_raw_parts(buffer, buffer_len as uint);
                f(&name[..], buf);
            }
        }
        unsafe {
            ::LLVMRustArchiveReadAllChildren(self.ptr,
                                             cb::<F>,
                                             transmute(&f));
        }
    }
}

impl Drop for ArchiveRO {
    fn drop(&mut self) {
        unsafe {
            ::LLVMRustDestroyArchive(self.ptr);
        }
    }
}
