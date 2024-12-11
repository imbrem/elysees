use core::borrow::Borrow;
use core::ffi::c_void;
use core::hash::{Hash, Hasher};
use core::ops::Deref;
use core::ptr;
use core::ptr::NonNull;
use core::sync::atomic;
use core::{cmp::Ordering, marker::PhantomData};
use core::{fmt, mem};

use erasable::{Erasable, ErasablePtr};

use super::{Arc, ArcInner, ArcRef};

/// A "borrowed [`Arc`]". This is essentially a reference to an `ArcInner<T>`
///
/// This is equivalent in guarantees to [`&Arc<T>`][`Arc`], however it has the same representation as an [`Arc<T>`], minimizing pointer-chasing.
///
/// [`ArcBorrow`] lets us deal with borrows of known-refcounted objects
/// without needing to worry about where the [`Arc<T>`][`Arc`] is.
#[repr(transparent)]
pub struct ArcBorrow<'a, T: ?Sized + 'a> {
    pub(crate) p: ptr::NonNull<T>,
    pub(crate) phantom: PhantomData<&'a T>,
}

impl<'a, T: ?Sized> Copy for ArcBorrow<'a, T> {}
impl<'a, T: ?Sized> Clone for ArcBorrow<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: ?Sized> ArcBorrow<'a, T> {
    /// Clone this as an [`Arc<T>`]. This bumps the refcount.
    #[inline]
    pub fn clone_arc(this: Self) -> Arc<T> {
        let arc = unsafe { Arc::from_raw(this.p.as_ptr()) };
        // addref it!
        mem::forget(arc.clone());
        arc
    }

    /// Compare two [`ArcBorrow`]s via pointer equality. Will only return
    /// true if they come from the same allocation
    #[inline]
    pub fn ptr_eq(this: Self, other: Self) -> bool {
        core::ptr::eq(this.p.as_ptr(), other.p.as_ptr())
    }

    /// Similar to deref, but uses the lifetime `'a` rather than the lifetime of
    /// `self`, which is incompatible with the signature of the [`Deref`] trait.
    #[inline]
    pub fn get(&self) -> &'a T {
        unsafe { &*(self.p.as_ptr() as *const T) }
    }

    /// Borrow this as an [`Arc`]. This does *not* bump the refcount.
    #[inline]
    pub fn as_arc(this: &Self) -> &Arc<T> {
        unsafe { &*(this as *const _ as *const Arc<T>) }
    }

    /// Get the internal pointer of an [`ArcBorrow`]
    #[inline]
    pub fn into_raw(this: Self) -> *const T {
        let arc = Self::as_arc(&this);
        Arc::as_ptr(arc)
    }

    /// Construct an [`ArcBorrow`] from an internal pointer
    ///
    /// # Safety
    /// This pointer must be the result of `ArcBorrow::from_raw` or `Arc::from_raw`. In the latter case, the reference count is not incremented.
    #[inline]
    pub unsafe fn from_raw(raw: *const T) -> Self {
        ArcBorrow {
            p: NonNull::new_unchecked(raw as *mut T),
            phantom: PhantomData,
        }
    }

    /// Gets the number of [`Arc`] pointers to this allocation
    #[inline]
    pub fn count(this: Self) -> usize {
        ArcBorrow::load_count(this, atomic::Ordering::Acquire)
    }

    /// Gets the number of [`Arc`] pointers to this allocation, with a given load ordering
    #[inline]
    pub fn load_count(this: Self, order: atomic::Ordering) -> usize {
        unsafe {
            (*(ArcInner::count_ptr(this.p.as_ptr()) as *const atomic::AtomicUsize)).load(order)
        }
    }

    /// Returns the address on the heap of the [`ArcRef`] itself -- not the `T` within it -- for memory
    /// reporting.
    pub fn heap_ptr(self) -> *const c_void {
        unsafe { ArcInner::count_ptr(self.p.as_ptr()) as *const c_void }
    }
}

impl<'a, T> ArcBorrow<'a, T> {
    /// Borrow this as an [`ArcRef`]. This does *not* bump the refcount.
    #[inline]
    pub fn as_arc_ref(this: &'a ArcBorrow<'a, T>) -> &'a ArcRef<'a, T> {
        unsafe { &*(this as *const _ as *const ArcRef<'a, T>) }
    }
}

impl<'a, T> Deref for ArcBorrow<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.get()
    }
}

unsafe impl<T: ?Sized + Erasable> ErasablePtr for ArcBorrow<'_, T> {
    #[inline]
    fn erase(this: Self) -> erasable::ErasedPtr {
        T::erase(unsafe { ptr::NonNull::new_unchecked(ArcBorrow::into_raw(this) as *mut _) })
    }

    #[inline]
    unsafe fn unerase(this: erasable::ErasedPtr) -> Self {
        ArcBorrow::from_raw(T::unerase(this).as_ptr())
    }
}

impl<'a, 'b, T, U: PartialEq<T>> PartialEq<ArcBorrow<'a, T>> for ArcBorrow<'b, U> {
    #[inline]
    fn eq(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) == *(*other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    #[inline]
    fn ne(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) != *(*other)
    }
}

impl<'a, 'b, T, U: PartialOrd<T>> PartialOrd<ArcBorrow<'a, T>> for ArcBorrow<'b, U> {
    #[inline]
    fn partial_cmp(&self, other: &ArcBorrow<'a, T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }

    #[inline]
    fn lt(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) < *(*other)
    }

    #[inline]
    fn le(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) <= *(*other)
    }

    #[inline]
    fn gt(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) > *(*other)
    }

    #[inline]
    fn ge(&self, other: &ArcBorrow<'a, T>) -> bool {
        *(*self) >= *(*other)
    }
}

impl<'a, T: Ord> Ord for ArcBorrow<'a, T> {
    #[inline]
    fn cmp(&self, other: &ArcBorrow<'a, T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<'a, T: Eq> Eq for ArcBorrow<'a, T> {}

impl<'a, T: fmt::Display> fmt::Display for ArcBorrow<'a, T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for ArcBorrow<'a, T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Hash> Hash for ArcBorrow<'_, T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<T> Borrow<T> for ArcBorrow<'_, T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> AsRef<T> for ArcBorrow<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn borrow_count() {
        let mut borrows = alloc::vec::Vec::with_capacity(100);
        let x = Arc::new(76);
        let y = Arc::borrow_arc(&x);
        assert_eq!(Arc::count(&x), 1);
        assert_eq!(ArcBorrow::count(y), 1);
        for i in 0..100 {
            borrows.push(x.clone());
            assert_eq!(Arc::count(&x), i + 2);
            assert_eq!(ArcBorrow::count(y), i + 2);
        }
    }
}
