use core::fmt;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr;

use super::{Arc, OffsetArcBorrow};

/// An [`Arc`], except it holds a pointer to the `T` instead of to the
/// entire [`ArcInner`](crate::ArcInner).
///
/// An [`OffsetArc<T>`][`OffsetArc`] has the same layout and ABI as a non-null
/// `const T*` in C, and may be used in FFI function signatures.
///
/// ```text
///  Arc<T>    OffsetArc<T>
///   |          |
///   v          v
///  ---------------------
/// | RefCount | T (data) | [ArcInner<T>]
///  ---------------------
/// ```
///
/// This means that this is a direct pointer to
/// its contained data (and can be read from by both C++ and Rust),
/// but we can also convert it to a "regular" [`Arc<T>`][`Arc`] by removing the offset.
///
/// This is very useful if you have an [`Arc`]-containing struct shared between Rust and C++,
/// and wish for C++ to be able to read the data behind the [`Arc`] without incurring
/// an FFI call overhead.
#[derive(Eq)]
#[repr(transparent)]
pub struct OffsetArc<T> {
    pub(crate) p: ptr::NonNull<T>,
    pub(crate) phantom: PhantomData<T>,
}

unsafe impl<T: Sync + Send> Send for OffsetArc<T> {}
unsafe impl<T: Sync + Send> Sync for OffsetArc<T> {}

impl<T> Deref for OffsetArc<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.p.as_ptr() }
    }
}

impl<T> Clone for OffsetArc<T> {
    #[inline]
    fn clone(&self) -> Self {
        Arc::into_raw_offset(Self::clone_arc(self))
    }
}

impl<T> Drop for OffsetArc<T> {
    #[inline]
    fn drop(&mut self) {
        let _ = Arc::from_raw_offset(OffsetArc {
            p: self.p,
            phantom: PhantomData,
        });
    }
}

impl<T: fmt::Debug> fmt::Debug for OffsetArc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: PartialEq> PartialEq for OffsetArc<T> {
    #[inline]
    fn eq(&self, other: &OffsetArc<T>) -> bool {
        *(*self) == *(*other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    #[inline]
    fn ne(&self, other: &OffsetArc<T>) -> bool {
        *(*self) != *(*other)
    }
}

impl<T> OffsetArc<T> {
    /// Temporarily converts `self` into a bonafide [`Arc`] and exposes it to the
    /// provided callback. The refcount is not modified.
    #[inline]
    pub fn with_arc<F, U>(this: &Self, f: F) -> U
    where
        F: FnOnce(&Arc<T>) -> U,
    {
        // Synthesize transient Arc, which never touches the refcount of the ArcInner.
        let transient = unsafe { ManuallyDrop::new(Arc::from_raw(this.p.as_ptr())) };

        // Expose the transient Arc to the callback, which may clone it if it wants
        // and forward the result to the user
        f(&transient)
    }

    /// If uniquely owned, provide a mutable reference
    /// Else create a copy, and mutate that
    ///
    /// This is functionally the same thing as [`Arc::make_mut`]
    #[inline]
    pub fn make_mut(this: &mut Self) -> &mut T
    where
        T: Clone,
    {
        unsafe {
            // extract the OffsetArc as an owned variable
            let this_ = ptr::read(this);
            // treat it as a real Arc
            let mut arc = Arc::from_raw_offset(this_);
            // obtain the mutable reference. Cast away the lifetime
            // This may mutate `arc`
            let ret = Arc::make_mut(&mut arc) as *mut _;
            // Store the possibly-mutated arc back inside, after converting
            // it to a OffsetArc again
            ptr::write(this, Arc::into_raw_offset(arc));
            &mut *ret
        }
    }

    /// Clone it as an [`Arc`]
    #[inline]
    pub fn clone_arc(this: &Self) -> Arc<T> {
        OffsetArc::with_arc(this, |a| a.clone())
    }

    /// Produce a pointer to the data that can be converted back
    /// to an [`Arc`]
    #[inline]
    pub fn borrow_arc(this: &Self) -> OffsetArcBorrow<'_, T> {
        OffsetArcBorrow {
            p: this.p,
            phantom: PhantomData,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn offset_round_trip() {
        let x = Arc::new(6453);
        let mut o = Arc::into_raw_offset(x.clone());
        let ob = OffsetArc::borrow_arc(&o);
        assert_eq!(*x, *o);
        assert_eq!(*x, *ob);
        let c = OffsetArc::clone_arc(&o);
        assert_eq!(*x, *c);
        assert_eq!(Arc::count(&x), 3);
        let om = OffsetArc::make_mut(&mut o);
        *om = 5;
        assert_eq!(*o, 5);
        assert_eq!(Arc::count(&x), 2);
    }
}