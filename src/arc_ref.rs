use core::borrow;
use core::cmp::Ordering;
use core::convert::From;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::ops::Deref;
use core::sync::atomic;
use erasable::{Erasable, ErasedPtr};
use std::ptr::NonNull;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "stable_deref_trait")]
use stable_deref_trait::{CloneStableDeref, StableDeref};

use crate::{Arc, ArcBorrow, ArcInner};

/// A *thin* atomically reference counted shared pointer, which may hold either exactly 0 references (in which case it is analogous to an [`ArcBorrow`])
/// or 1 (in which case it is analogous to an [`Arc`])
///
/// See the documentation for [`Arc`][aa] in the standard library. Unlike the
/// standard library [`Arc`][aa], this [`Arc`] does not support weak reference counting.
///
/// [aa]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html
#[repr(transparent)]
pub struct ArcRef<'a, T: Erasable> {
    pub(crate) p: ErasedPtr,
    pub(crate) phantom: PhantomData<&'a T>,
}

unsafe impl<'a, T: Erasable + Sync + Send> Send for ArcRef<'a, T> {}
unsafe impl<'a, T: Erasable + Sync + Send> Sync for ArcRef<'a, T> {}

impl<'a, T: Erasable> ArcRef<'a, T> {
    /// Construct an [`ArcRef<'a, T>`]
    #[inline]
    pub fn new(data: T) -> Self {
        ArcRef::from_arc(Arc::new(data))
    }

    /// Returns the inner value, if the [`ArcRef`] is owned and has exactly one strong reference.
    ///
    /// Otherwise, an [`Err`] is returned with the same [`ArcRef`] that was
    /// passed in.
    ///
    /// # Examples
    ///
    /// ```
    /// use elysees::Arc;
    ///
    /// let x = Arc::new(3);
    /// assert_eq!(Arc::try_unwrap(x), Ok(3));
    ///
    /// let x = Arc::new(4);
    /// let _y = Arc::clone(&x);
    /// assert_eq!(*Arc::try_unwrap(x).unwrap_err(), 4);
    /// ```
    #[inline]
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        match ArcRef::try_into_arc(this) {
            Ok(arc) => Arc::try_unwrap(arc).map_err(ArcRef::from_arc),
            Err(borrow) => Err(ArcRef::from_borrow(borrow)),
        }
    }
}

//TODO: support unsized types for functions in this block
impl<'a, T: Erasable> ArcRef<'a, T> {
    /// Makes a mutable reference to the [`Arc`], cloning if necessary
    ///
    /// This is functionally equivalent to [`Arc::make_mut`][mm] from the standard library.
    ///
    /// If this [`Arc`] is uniquely owned, `make_mut()` will provide a mutable
    /// reference to the contents. If not, `make_mut()` will create a _new_ [`Arc`]
    /// with a copy of the contents, update `this` to point to it, and provide
    /// a mutable reference to its contents.
    ///
    /// This is useful for implementing copy-on-write schemes where you wish to
    /// avoid copying things if your [`Arc`] is not shared.
    ///
    /// [mm]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html#method.make_mut
    #[inline]
    pub fn make_mut(this: &mut Self) -> &mut T
    where
        T: Clone,
    {
        if !this.is_unique() {
            // Another pointer exists; clone
            *this = ArcRef::new((**this).clone());
        }

        unsafe {
            // This unsafety is ok because we're guaranteed that the pointer
            // returned is the *only* pointer that will ever be returned to T. Our
            // reference count is guaranteed to be 1 at this point, and we required
            // the Arc itself to be `mut`, so we're returning the only possible
            // reference to the inner data.
            &mut (*this.ptr()).data
        }
    }

    /// Whether or not the [`Arc`] is uniquely owned (is the refcount 1?).
    #[inline]
    pub fn is_unique(&self) -> bool {
        // See the extensive discussion in [1] for why this needs to be Acquire.
        //
        // [1] https://github.com/servo/servo/issues/21186
        Self::count(self) == 1
    }

    /// Gets the number of [`Arc`] pointers to this allocation
    #[inline]
    pub fn count(this: &Self) -> usize {
        Self::load_count(this, atomic::Ordering::Acquire)
    }

    /// Gets the number of [`Arc`] pointers to this allocation, with a given load ordering
    #[inline]
    pub fn load_count(this: &Self, order: atomic::Ordering) -> usize {
        this.inner().count.load(order)
    }

    /// Construct an `ArcRef<'a, T>` from an `Arc<T>`
    #[inline]
    pub fn from_arc(arc: Arc<T>) -> Self {
        ArcRef {
            p: Erasable::erase(unsafe {
                NonNull::new_unchecked((Erasable::erase(arc.p).as_ptr() as usize | 0b10) as *mut u8)
            }),
            phantom: PhantomData,
        }
    }

    /// Construct an `ArcRef<'a, T>` from an `ArcBorrow<'a, T>`
    #[inline]
    pub fn from_borrow(arc: ArcBorrow<'a, T>) -> Self {
        ArcRef {
            p: Erasable::erase(arc.p),
            phantom: PhantomData,
        }
    }

    /// Try to get this `ArcRef<'a, T>` as an `Arc<T>`
    #[inline]
    pub fn try_into_arc(this: Self) -> Result<Arc<T>, ArcBorrow<'a, T>> {
        let p = this.nn_ptr();
        let owned = ArcRef::is_owned(&this);
        core::mem::forget(this);
        if owned {
            Ok(Arc {
                p,
                phantom: PhantomData,
            })
        } else {
            Err(ArcBorrow {
                p,
                phantom: PhantomData,
            })
        }
    }

    /// Convert this `ArcRef<'a, T>` into an `Arc<T>`
    #[inline]
    pub fn into_arc(this: Self) -> Arc<T> {
        match ArcRef::try_into_arc(this) {
            Ok(arc) => arc,
            Err(borrow) => borrow.clone_arc(),
        }
    }

    #[inline]
    pub(super) fn inner(&self) -> &ArcInner<T> {
        // This unsafety is ok because while this arc is alive we're guaranteed
        // that the inner pointer is valid. Furthermore, we know that the
        // `ArcInner` structure itself is `Sync` because the inner data is
        // `Sync` as well, so we're ok loaning out an immutable pointer to these
        // contents.
        unsafe { &*self.ptr() }
    }

    /// Test pointer equality between the two [`Arc`]s, i.e. they must be the _same_
    /// allocation
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.nn_ptr() == other.nn_ptr()
    }

    #[inline]
    pub(crate) fn nn_ptr(&self) -> NonNull<ArcInner<T>> {
        let buf_ptr = (self.p.as_ptr() as usize & !0b11) as *mut u8;
        let erased = unsafe { Erasable::erase(NonNull::new_unchecked(buf_ptr)) };
        unsafe { Erasable::unerase(erased) }
    }

    #[inline]
    pub(crate) fn ptr(&self) -> *mut ArcInner<T> {
        self.nn_ptr().as_ptr()
    }

    /// Leak this [`Arc<T>`][`Arc`], getting an [`ArcBorrow<'static, T>`][`ArcBorrow`]
    ///
    /// You can call the [`get`][`ArcBorrow::get`] method on the returned [`ArcBorrow`] to get an `&'static T`.
    /// Note that using this can (obviously) cause memory leaks!
    #[inline]
    pub fn leak(this: ArcRef<T>) -> ArcBorrow<'static, T> {
        let result = ArcBorrow {
            p: this.nn_ptr(),
            phantom: PhantomData,
        };
        mem::forget(this);
        result
    }

    /// Get whether this `ArcRef<'a, T>` is owned
    #[inline]
    pub fn is_owned(this: &Self) -> bool {
        this.p.as_ptr() as usize | 0b10 != 0
    }

    /// Borrow this as an [`ArcBorrow`]. This does *not* bump the refcount.
    #[inline]
    pub fn borrow_arc(&'a self) -> ArcBorrow<'a, T> {
        ArcBorrow {
            p: self.nn_ptr(),
            phantom: PhantomData,
        }
    }

    /// Clone this as an [`Arc`].
    #[inline]
    pub fn clone_arc(&'a self) -> Arc<T> {
        self.borrow_arc().clone_arc()
    }

    /// Get this as an owned pointer
    #[inline]
    pub fn into_owned(this: Self) -> ArcRef<'static, T> {
        match Self::try_into_arc(this) {
            Ok(arc) => ArcRef::from_arc(arc),
            Err(borrow) => ArcRef::from_arc(borrow.clone_arc()),
        }
    }

    /// Get clone this into an owned pointer
    #[inline]
    pub fn clone_into_owned(this: &Self) -> ArcRef<'static, T> {
        ArcRef::from_arc(this.clone_arc())
    }

    /// Get the internal pointer of an [`ArcBorrow`]
    #[inline]
    pub fn into_raw(this: Self) -> *const T {
        ArcBorrow::into_raw(this.borrow_arc())
    }

    /// Get the internal pointer of an [`ArcBorrow`]
    #[inline]
    pub fn as_ptr(this: &Self) -> *const T {
        ArcBorrow::into_raw(this.borrow_arc())
    }
}

impl<'a, T: Erasable> Drop for ArcRef<'a, T> {
    #[inline]
    fn drop(&mut self) {
        if ArcRef::is_owned(self) {
            core::mem::drop(Arc {
                p: self.nn_ptr(),
                phantom: PhantomData,
            })
        }
    }
}

impl<'a, T: Erasable> Clone for ArcRef<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        if Self::is_owned(self) {
            Self::from_arc(self.clone_arc())
        } else {
            ArcRef {
                p: self.p,
                phantom: PhantomData,
            }
        }
    }
}

impl<'a, T: Erasable> Deref for ArcRef<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.inner().data
    }
}

impl<'a, 'b, T: Erasable, U: Erasable + PartialEq<T>> PartialEq<ArcRef<'a, T>> for ArcRef<'b, U> {
    fn eq(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) == *(*other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    fn ne(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) != *(*other)
    }
}

impl<'a, 'b, T: Erasable, U: Erasable + PartialOrd<T>> PartialOrd<ArcRef<'a, T>> for ArcRef<'b, U> {
    fn partial_cmp(&self, other: &ArcRef<'a, T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }

    fn lt(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) < *(*other)
    }

    fn le(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) <= *(*other)
    }

    fn gt(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) > *(*other)
    }

    fn ge(&self, other: &ArcRef<'a, T>) -> bool {
        *(*self) >= *(*other)
    }
}

impl<'a, T: Erasable + Ord> Ord for ArcRef<'a, T> {
    fn cmp(&self, other: &ArcRef<'a, T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<'a, T: Erasable + Eq> Eq for ArcRef<'a, T> {}

impl<'a, T: Erasable + fmt::Display> fmt::Display for ArcRef<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: Erasable + fmt::Debug> fmt::Debug for ArcRef<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: Erasable + fmt::Pointer> fmt::Pointer for ArcRef<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr(), f)
    }
}

impl<'a, T: Erasable + Default> Default for ArcRef<'a, T> {
    #[inline]
    fn default() -> ArcRef<'a, T> {
        ArcRef::new(Default::default())
    }
}

impl<'a, T: Erasable + Hash> Hash for ArcRef<'a, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<'a, T> From<T> for ArcRef<'a, T> {
    #[inline]
    fn from(t: T) -> Self {
        ArcRef::new(t)
    }
}

impl<'a, T: Erasable> borrow::Borrow<T> for ArcRef<'a, T> {
    #[inline]
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<'a, T: Erasable> AsRef<T> for ArcRef<'a, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        &**self
    }
}

#[cfg(feature = "stable_deref_trait")]
unsafe impl<'a, T: Erasable> StableDeref for ArcRef<'a, T> {}
#[cfg(feature = "stable_deref_trait")]
unsafe impl<'a, T: Erasable> CloneStableDeref for ArcRef<'a, T> {}

#[cfg(feature = "serde")]
impl<'a, 'de, T: Deserialize<'de>> Deserialize<'de> for ArcRef<'a, T> {
    fn deserialize<D>(deserializer: D) -> Result<ArcRef<'a, T>, D::Error>
    where
        D: ::serde::de::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(ArcRef::new)
    }
}

#[cfg(feature = "serde")]
impl<'a, T: Serialize> Serialize for ArcRef<'a, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ::serde::ser::Serializer,
    {
        (**self).serialize(serializer)
    }
}