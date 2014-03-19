// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use ai = std::io::net::addrinfo;
use std::c_str::CString;
use std::cast;
use std::io::IoError;
use std::libc;
use std::libc::{c_char, c_int};
use std::ptr::{null, mut_null};

use super::net::sockaddr_to_addr;

pub struct GetAddrInfoRequest;

impl GetAddrInfoRequest {
    pub fn run(host: Option<&str>, servname: Option<&str>,
               hint: Option<ai::Hint>) -> Result<~[ai::Info], IoError> {
        assert!(host.is_some() || servname.is_some());

        let c_host = host.map_or(unsafe { CString::new(null(), true) }, |x| x.to_c_str());
        let c_serv = servname.map_or(unsafe { CString::new(null(), true) }, |x| x.to_c_str());

        let hint = hint.map(|hint| {
            libc::addrinfo {
                ai_flags: hint.flags as c_int,
                ai_family: hint.family as c_int,
                ai_socktype: 0,
                ai_protocol: 0,
                ai_addrlen: 0,
                ai_canonname: null(),
                ai_addr: null(),
                ai_next: null()
            }
        });

        let hint_ptr = hint.as_ref().map_or(null(), |x| x as *libc::addrinfo);
        let mut res = mut_null();

        // Make the call
        let s = unsafe {
            let ch = if c_host.is_null() { null() } else { c_host.with_ref(|x| x) };
            let cs = if c_serv.is_null() { null() } else { c_serv.with_ref(|x| x) };
            getaddrinfo(ch, cs, hint_ptr, &mut res)
        };

        // Error?
        if s != 0 {
            return Err(get_error(s));
        }

        // Collect all the results we found
        let mut addrs = ~[];
        let mut rp = res;
        while rp.is_not_null() {
            unsafe {
                let addr = match sockaddr_to_addr(cast::transmute((*rp).ai_addr),
                                                  (*rp).ai_addrlen as uint) {
                    Ok(a) => a,
                    Err(e) => return Err(e)
                };
                addrs.push(ai::Info {
                    address: addr,
                    family: (*rp).ai_family as uint,
                    socktype: None,
                    protocol: None,
                    flags: (*rp).ai_flags as uint
                });

                rp = (*rp).ai_next as *mut libc::addrinfo;
            }
        }

        unsafe { freeaddrinfo(res); }

        Ok(addrs)
    }
}

#[cfg(not(target_os = "nacl", target_libc = "newlib"))]
extern "system" {
    fn getaddrinfo(node: *c_char, service: *c_char,
                   hints: *libc::addrinfo, res: *mut *mut libc::addrinfo) -> c_int;
    fn freeaddrinfo(res: *mut libc::addrinfo);
    #[cfg(not(windows))]
    fn gai_strerror(errcode: c_int) -> *c_char;
    #[cfg(windows)]
    fn WSAGetLastError() -> c_int;
}
#[cfg(target_os = "nacl", target_libc = "newlib")]
unsafe fn getaddrinfo(_node: *c_char, _service: *c_char,
                      _hints: *libc::addrinfo, _res: *mut *mut libc::addrinfo) -> c_int {
    -1 as i32
}
#[cfg(target_os = "nacl", target_libc = "newlib")]
unsafe fn freeaddrinfo(_res: *mut libc::addrinfo) {}
#[cfg(target_os = "nacl", target_libc = "newlib")]
unsafe fn gai_strerror(_errcode: c_int) -> *c_char {
    static EMPTY: &'static str = "";
    EMPTY.as_ptr() as *c_char
}

#[cfg(windows)]
fn get_error(_: c_int) -> IoError {
    use super::translate_error;

    unsafe {
        translate_error(WSAGetLastError() as i32, true)
    }
}

#[cfg(not(windows))]
fn get_error(s: c_int) -> IoError {
    use std::io;
    use std::str::raw::from_c_str;

    let err_str = unsafe { from_c_str(gai_strerror(s)) };
    IoError {
        kind: io::OtherIoError,
        desc: "unable to resolve host",
        detail: Some(err_str),
    }
}
