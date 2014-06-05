// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![deny(experimental)]
#![feature(phase)]

#[phase(syntax)] extern crate simd_syntax;

extern crate simd;
use simd::{i64x2, Simd};

fn main() {
    let a: i64x2 = gather_simd!(0, 0);
    let _ = a.all(0); //~ ERROR: experimental
}
