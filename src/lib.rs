// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Fork of [`triomphe`](https://github.com/Manishearth/triomphe/), which is a fork of [`Arc`][std::sync::Arc]. This has the following advantages over [`std::sync::Arc`]:
//!
//! * [`elysees::Arc`][`Arc`] doesn't support weak references: we save space by excluding the weak reference count, and we don't do extra read-modify-update operations to handle the possibility of weak references.
//! * [`elysees::ArcBox`][`ArcBox`] allows one to construct a temporarily-mutable [`Arc`] which can be converted to a regular [`elysees::Arc`][`Arc`] later
//! * [`elysees::OffsetArc`][`OffsetArc`] can be used transparently from C++ code and is compatible with (and can be converted to/from) [`elysees::Arc`][`Arc`]
//! * [`elysees::ArcBorrow`][`ArcBorrow`] is functionally similar to [`&elysees::Arc<T>`][`Arc`], however in memory it's simply a (non-owned) pointer to the inner [`Arc`]. This helps avoid pointer-chasing.
//! * [`elysees::OffsetArcBorrow`][`OffsetArcBorrow`] is functionally similar to [`&Arc<T>`][`Arc`], however in memory it's simply `&T`. This makes it more flexible for FFI; the source of the borrow need not be an [`Arc`] pinned on the stack (and can instead be a pointer from C++, or an [`OffsetArc`]). Additionally, this helps avoid pointer-chasing.
//! * [`elysees::ArcRef`][`ArcRef`] is a union of an [`Arc`] and an [`ArcBorrow`]

#![allow(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate core;

#[macro_use]
extern crate memoffset;
#[cfg(feature = "arc-swap")]
extern crate arc_swap;
#[cfg(feature = "serde")]
extern crate serde;
#[cfg(feature = "stable_deref_trait")]
extern crate stable_deref_trait;
#[cfg(feature = "unsize")]
extern crate unsize;

mod arc;
mod arc_borrow;
#[cfg(feature = "arc-swap")]
mod arc_swap_support;
mod offset_arc;
mod unique_arc;

pub use arc::*;
pub use arc_borrow::*;
pub use offset_arc::*;
pub use unique_arc::*;

#[cfg(feature = "std")]
use std::process::abort;

// `no_std`-compatible abort by forcing a panic while already panicing.
#[cfg(not(feature = "std"))]
#[cold]
fn abort() -> ! {
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!()
        }
    }
    let _double_panicer = PanicOnDrop;
    panic!();
}
