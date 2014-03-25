// Copyright 2012-2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// force-host
// xfail-stage1

#[feature(simd, phase)];
#[allow(experimental)];

#[phase(syntax)]
extern crate simd_syntax;
extern crate simd;

use simd::{f32x4, BoolSimd};

static G1: f32x4 = gather_simd!(1.0, 2.0, 3.0, 4.0);
static G2: f32x4 = swizzle_simd!(G1 -> (3, 2, 1, 0));

pub fn main() {
    let c = swizzle_simd!(G2 -> (3, 2, 1, 0)) == G1;
    assert!(c.all_true());
}
