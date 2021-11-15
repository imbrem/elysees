use core::mem;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr;
use std::marker::PhantomData;
use std::sync::atomic;

use super::{Arc, ArcInner, OffsetArc};

/// A "borrowed [`Arc`]". This is essentially a reference to an `ArcInner<T>`
///
/// This is equivalent in guarantees to [`&Arc<T>`][`Arc`], however it has the same representation as an [`Arc<T>`], minimizing pointer-chasing.
///
/// [`ArcBorrow`] lets us deal with borrows of known-refcounted objects
/// without needing to worry about where the [`Arc<T>`][`Arc`] is.
#[derive(Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct ArcBorrow<'a, T: ?Sized + 'a> {
    pub(crate) p: ptr::NonNull<ArcInner<T>>,
    pub(crate) phantom: PhantomData<&'a T>,
}

impl<'a, T> Copy for ArcBorrow<'a, T> {}
impl<'a, T> Clone for ArcBorrow<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> ArcBorrow<'a, T> {
    /// Clone this as an [`Arc<T>`]. This bumps the refcount.
    #[inline]
    pub fn clone_arc(&self) -> Arc<T> {
        let arc = Arc {
            p: self.p,
            phantom: PhantomData,
        };
        // addref it!
        mem::forget(arc.clone());
        arc
    }

    /// Compare two [`ArcBorrow`]s via pointer equality. Will only return
    /// true if they come from the same allocation
    #[inline]
    pub fn ptr_eq(this: Self, other: Self) -> bool {
        this.p == other.p
    }

    /// Similar to deref, but uses the lifetime `'a` rather than the lifetime of
    /// `self`, which is incompatible with the signature of the [`Deref`] trait.
    #[inline]
    pub fn get(&self) -> &'a T {
        &self.inner().data
    }

    /// Borrow this as an [`Arc`]. This does *not* bump the refcount.
    #[inline]
    pub fn as_arc(&self) -> &Arc<T> {
        unsafe { &*(self as *const _ as *const Arc<T>) }
    }

    /// Get the internal pointer of an [`ArcBorrow`]
    #[inline]
    pub fn into_raw(this: Self) -> *const T {
        this.as_arc().as_ptr()
    }

    #[inline]
    pub(super) fn inner(&self) -> &'a ArcInner<T> {
        // This unsafety is ok because while this arc is alive we're guaranteed
        // that the inner pointer is valid. Furthermore, we know that the
        // `ArcInner` structure itself is `Sync` because the inner data is
        // `Sync` as well, so we're ok loaning out an immutable pointer to these
        // contents.
        unsafe { &*self.p.as_ptr() }
    }

    /// Gets the number of [`Arc`] pointers to this allocation
    #[inline]
    pub fn count(this: Self) -> usize {
        ArcBorrow::load_count(this, atomic::Ordering::Acquire)
    }

    /// Gets the number of [`Arc`] pointers to this allocation, with a given load ordering
    #[inline]
    pub fn load_count(this: Self, order: atomic::Ordering) -> usize {
        this.inner().count.load(order)
    }
}

impl<'a, T> Deref for ArcBorrow<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.get()
    }
}

/// A "borrowed [`OffsetArc`]". This is a pointer to
/// a T that is known to have been allocated within an
/// [`Arc`].
///
/// This is equivalent in guarantees to [`&Arc<T>`][`Arc`], however it is
/// a bit more flexible. To obtain an [`&Arc<T>`][`Arc`] you must have
/// an [`Arc<T>`][`Arc`] instance somewhere pinned down until we're done with it.
/// It's also a direct pointer to `T`, so using this involves less pointer-chasing
///
/// However, C++ code may hand us refcounted things as pointers to `T` directly,
/// so we have to conjure up a temporary [`Arc`] on the stack each time. The
/// same happens for when the object is managed by a [`OffsetArc`].
///
/// [`OffsetArcBorrow`] lets us deal with borrows of known-refcounted objects
/// without needing to worry about where the [`Arc<T>`] is.
#[derive(Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct OffsetArcBorrow<'a, T: ?Sized + 'a> {
    pub(crate) p: ptr::NonNull<T>,
    pub(crate) phantom: PhantomData<&'a T>,
}

impl<'a, T> Copy for OffsetArcBorrow<'a, T> {}
impl<'a, T> Clone for OffsetArcBorrow<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> OffsetArcBorrow<'a, T> {
    /// Clone this as an [`Arc<T>`]. This bumps the refcount.
    #[inline]
    pub fn clone_arc(&self) -> Arc<T> {
        let arc = unsafe { Arc::from_raw(self.p.as_ptr()) };
        // addref it!
        mem::forget(arc.clone());
        arc
    }

    /// Compare two [`ArcBorrow`]s via pointer equality. Will only return
    /// true if they come from the same allocation
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.p == other.p
    }

    /// Temporarily converts `self` into a bonafide [`Arc`] and exposes it to the
    /// provided callback. The refcount is not modified.
    #[inline]
    pub fn with_arc<F, U>(&self, f: F) -> U
    where
        F: FnOnce(&Arc<T>) -> U,
        T: 'static,
    {
        // Synthesize transient Arc, which never touches the refcount.
        let transient = unsafe { ManuallyDrop::new(Arc::from_raw(self.p.as_ptr())) };

        // Expose the transient Arc to the callback, which may clone it if it wants
        // and forward the result to the user
        f(&transient)
    }

    /// Borrow this as an [`OffsetArc`]. This does *not* bump the refcount.
    #[inline]
    pub fn as_arc(&self) -> &OffsetArc<T> {
        unsafe { &*(self as *const _ as *const OffsetArc<T>) }
    }

    /// Similar to deref, but uses the lifetime `'a` rather than the lifetime of
    /// `self`, which is incompatible with the signature of the [`Deref`] trait.
    #[inline]
    pub fn get(&self) -> &'a T {
        unsafe { &*self.p.as_ptr() }
    }
}

impl<'a, T> Deref for OffsetArcBorrow<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.get()
    }
}
