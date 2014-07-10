// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![crate_id = "simd#0.11-pre"]
#![crate_type = "dylib"]
#![crate_type = "rlib"]
#![license = "MIT/ASL2"]
#![comment = "A link-time library to facilitate access to SIMD types & operations"]

#![feature(simd, phase, macro_rules)]
#![allow(experimental)]
#![experimental]

#[phase(plugin)]
extern crate simd_syntax;

use std::iter;
//use std::collections::Collection;
use std::mem;

#[experimental]
pub trait Simd<PrimitiveTy> {
    #[experimental] fn smear(value: PrimitiveTy) -> Self;
    #[experimental] fn all(self, value: PrimitiveTy) -> bool;
    #[experimental] fn any(self, value: PrimitiveTy) -> bool;

    #[experimental] fn iter<'a>(&'a self) -> Items<'a, PrimitiveTy>;
    #[experimental] fn mut_iter<'a>(&'a mut self) -> MutItems<'a, PrimitiveTy>;

    #[experimental] fn as_slice<'a>(&'a self) -> &'a [PrimitiveTy];
    #[experimental] fn as_mut_slice<'a>(&'a mut self) -> &'a mut [PrimitiveTy];

    #[experimental] fn len(&self) -> uint;
}
#[experimental]
pub trait BoolSimd {
    #[experimental] fn all_true(self) -> bool;
    #[experimental] fn all_false(self) -> bool;
    #[experimental] fn any_true(self) -> bool;
    #[experimental] fn any_false(self) -> bool;
}
impl<T: Simd<bool>> BoolSimd for T {
    #[experimental] #[inline] fn all_true(self) -> bool { self.all(true) }
    #[experimental] #[inline] fn all_false(self) -> bool { self.all(false) }
    #[experimental] #[inline] fn any_true(self) -> bool { self.any(true) }
    #[experimental] #[inline] fn any_false(self) -> bool { self.any(false) }
}
#[allow(raw_pointer_deriving)]
#[deriving(PartialEq, Eq, Clone)]
pub struct Items<'a, ElemT> {
    vec: *const ElemT,
    pos: uint,
    len: uint,
}
#[allow(raw_pointer_deriving)]
#[deriving(PartialEq, Eq, Clone)]
pub struct MutItems<'a, ElemT> {
    vec: *mut ElemT,
    pos: uint,
    len: uint,
}

impl<'a, ElemT> iter::Iterator<&'a ElemT> for Items<'a, ElemT> {
    fn next(&mut self) -> Option<&'a ElemT> {
        if self.pos >= self.len {
            None
        } else {
            self.pos += 1;
            Some(unsafe { mem::transmute(self.vec.offset((self.pos - 1) as int)) })
        }
    }
}
impl<'a, ElemT> iter::Iterator<&'a mut ElemT> for MutItems<'a, ElemT> {
    fn next(&mut self) -> Option<&'a mut ElemT> {
        if self.pos >= self.len {
            None
        } else {
            self.pos += 1;
            Some(unsafe { mem::transmute(self.vec.offset((self.pos - 1) as int)) })
        }
    }
}

macro_rules! _def(
    ($ident:ident = ($prim:ty, ..$len:expr)) => {
        def_type_simd!( #[experimental]
                        #[allow(non_camel_case_types)]
                        pub type $ident = <$prim, ..$len>)
        impl Simd<$prim> for $ident {
            #[experimental] #[inline] fn smear(value: $prim) -> $ident {
                smear_simd!(value, ..$len)
            }
            #[experimental] #[inline] fn all(self, value: $prim) -> bool {
                for i in iter::range(0u, $len as uint) {
                    if self[i] != value {
                        return false;
                    }
                }
                return true;
            }
            #[experimental] #[inline] fn any(self, value: $prim) -> bool {
                for i in iter::range(0u, $len as uint) {
                    if self[i] == value {
                        return true;
                    }
                }
                return false;
            }

            #[experimental] fn iter<'a>(&'a self) -> Items<'a, $prim> {
                Items {
                    vec: unsafe { mem::transmute(self) },
                    pos: 0,
                    len: $len,
                }
            }
            #[experimental] fn mut_iter<'a>(&'a mut self) -> MutItems<'a, $prim> {
                MutItems {
                    vec: unsafe { mem::transmute(self) },
                    pos: 0,
                    len: $len,
                }
            }

            #[experimental] fn as_slice<'a>(&'a self) -> &'a [$prim] {
                use std::raw::Slice;
                unsafe {
                    mem::transmute_copy(&Slice {
                        data: self as *const $ident as *const $prim,
                        len: $len,
                    })
                }
            }
            #[experimental] fn as_mut_slice<'a>(&'a mut self) -> &'a mut [$prim] {
                use std::raw::Slice;
                unsafe {
                    mem::transmute_copy(&Slice {
                        data: mem::transmute::<*mut $prim, *const $prim>
                            (self as *mut $ident as *mut $prim),
                        len: $len,
                    })
                }
            }

            #[experimental] #[inline(always)] fn len(&self) -> uint { $len }
        }
    }
)
_def!(boolx2  = (bool, ..2 ))
_def!(boolx4  = (bool, ..4 ))
_def!(boolx8  = (bool, ..8 ))
_def!(boolx16 = (bool, ..16))
_def!(boolx32 = (bool, ..32))
_def!(boolx64 = (bool, ..64))

_def!(i8x16   = (i8,   ..16))
_def!(i8x32   = (i8,   ..32))
_def!(i8x64   = (i8,   ..64))
_def!(u8x16   = (u8,   ..16))
_def!(u8x32   = (u8,   ..32))
_def!(u8x64   = (u8,   ..64))

_def!(i16x8   = (i16,  ..8 ))
_def!(i16x16  = (i16,  ..16))
_def!(i16x32  = (i16,  ..32))
_def!(u16x8   = (u16,  ..8 ))
_def!(u16x16  = (u16,  ..16))
_def!(u16x32  = (u16,  ..32))

_def!(i32x4   = (i32,  ..4 ))
_def!(i32x8   = (i32,  ..8 ))
_def!(i32x16  = (i32,  ..16))
_def!(u32x4   = (u32,  ..4 ))
_def!(u32x8   = (u32,  ..8 ))
_def!(u32x16  = (u32,  ..16))

_def!(i64x2   = (i64,  ..2 ))
_def!(i64x4   = (i64,  ..4 ))
_def!(i64x8   = (i64,  ..8 ))
_def!(u64x2   = (u64,  ..2 ))
_def!(u64x4   = (u64,  ..4 ))
_def!(u64x8   = (u64,  ..8 ))

_def!(f32x4   = (f32,  ..4 ))
_def!(f32x8   = (f32,  ..8 ))
_def!(f32x16  = (f32,  ..16))
_def!(f64x2   = (f64,  ..2 ))
_def!(f64x4   = (f64,  ..4 ))
_def!(f64x8   = (f64,  ..8 ))

#[cfg(test)]
mod test {
    use super::Simd;
    #[test]
    fn as_slice() {
        let v: super::i32x4 = gather_simd!(1, 2, 3, 4);
        assert!(v.as_slice() == [1, 2, 3, 4]);

        let v: super::i32x4 = gather_simd!(1, 2, 3, 4);
        let v = swizzle_simd!(v -> (3, 2, 1, 0));
        assert!(v.as_slice() == [4, 3, 2, 1]);
    }
    #[test]
    fn as_mut_slice() {
        let mut v: super::i32x4 = gather_simd!(1, 2, 3, 4);
        assert!(v.as_slice() == [1, 2, 3, 4]);
        v.as_mut_slice()[0] = 10;
        assert!(v.as_slice() == [10, 2, 3, 4]);

        let v: super::i32x4 = gather_simd!(1, 2, 3, 4);
        let mut v = swizzle_simd!(v -> (3, 2, 1, 0));
        assert!(v.as_slice() == [4, 3, 2, 1]);
        v.as_mut_slice()[0] = 10;
        assert!(v.as_slice() == [10, 3, 2, 1]);
    }
}
