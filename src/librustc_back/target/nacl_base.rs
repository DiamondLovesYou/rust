// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use target::{Target, TargetOptions};
use std::default::Default;

pub fn base_target() -> Target {
    let opts = TargetOptions {
        dynamic_linking: false,
        executables: true,
        .. Default::default()
    };
    Target {
        data_layout: "e-i1:8:8-i8:8:8-i16:16:16-i32:32:32-\
                      i64:64:64-f32:32:32-f64:64:64-p:32:32:32-v128:32:32".to_string(),
        llvm_target: "".to_string(),
        target_endian: "".to_string(),
        target_word_size: "".to_string(),
        target_os: "nacl".to_string(),
        arch: "".to_string(),
        options: opts,
    }
}
