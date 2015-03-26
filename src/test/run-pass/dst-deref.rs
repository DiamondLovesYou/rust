// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// Test that a custom deref with a fat pointer return type does not ICE

// pretty-expanded FIXME #23616

use std::ops::Deref;

pub struct Arr {
    ptr: Box<[uint]>
}

impl Deref for Arr {
    type Target = [uint];

    fn deref(&self) -> &[uint] {
        &*self.ptr
    }
}

pub fn foo(arr: &Arr) {
    assert!(arr.len() == 3);
    let x: &[uint] = &**arr;
    assert!(x[0] == 1);
    assert!(x[1] == 2);
    assert!(x[2] == 3);
}

fn main() {
    // FIXME (#22405): Replace `Box::new` with `box` here when/if possible.
    let a = Arr { ptr: Box::new([1, 2, 3]) };
    foo(&a);
}
