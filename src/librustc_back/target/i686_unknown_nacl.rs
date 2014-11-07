// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::Target;
use super::nacl_base;

pub fn target() -> Target {
    let mut b = nacl_base::base_target();
    b.llvm_target = "i686-unknown-nacl".to_string();
    b.target_endian = "little".to_string();
    b.target_word_size = "32".to_string();
    b.arch = "i686".to_string();

    b.options.cpu = "core2".to_string();
    b.options.morestack = true;
    b.options.exe_suffix = "nexe".to_string();
    b
}
