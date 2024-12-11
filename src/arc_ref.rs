use core::borrow;
use core::cmp::Ordering;
use core::convert::From;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic;
use erasable::{Erasable, ErasedPtr};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "stable_deref_trait")]
use stable_deref_trait::{CloneStableDeref, StableDeref};

use crate::{Arc, ArcBorrow, ArcBox, ArcInner};

/// An atomically reference counted shared pointer, which may hold either exactly 0 references (in which case it is analogous to an [`ArcBorrow`])
/// or 1 (in which case it is analogous to an [`Arc`])
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
        let new = ArcRef::from_arc(Arc::new(data));
        new
    }

    /// Returns the inner value, if the [`ArcRef`] is owned and has exactly one strong reference.
    ///
    /// Otherwise, an [`Err`] is returned with the same [`ArcRef`] that was
    /// passed in.
    ///
    /// # Examples
    ///
    /// ```
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(3);
    /// assert_eq!(ArcRef::try_unwrap(x), Ok(3));
    ///
    /// let x = ArcRef::new(4);
    /// let _y = ArcRef::clone(&x);
    /// assert_eq!(*ArcRef::try_unwrap(x).unwrap_err(), 4);
    /// ```
    #[inline]
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        match ArcRef::try_into_arc(this) {
            Ok(arc) => Arc::try_unwrap(arc).map_err(ArcRef::from_arc),
            Err(borrow) => Err(ArcRef::from_borrow(borrow)),
        }
    }

    /// Makes a mutable reference to the [`ArcRef`], cloning if necessary.
    ///
    /// This is similar to [`ArcRef::make_mut`][mm] from the standard library.
    ///
    /// If this [`ArcRef`] is uniquely owned, `make_mut()` will provide a mutable
    /// reference to the contents. If not, `make_mut()` will create a _new_ [`ArcRef`]
    /// with a copy of the contents, update `this` to point to it, and provide
    /// a mutable reference to its contents.
    ///
    /// This is useful for implementing copy-on-write schemes where you wish to
    /// avoid copying things if your [`ArcRef`] is not shared.
    ///
    /// [mm]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html#method.make_mut
    #[inline]
    pub fn make_mut(this: &mut Self) -> &mut T
    where
        T: Clone,
    {
        if !ArcRef::is_unique(this) {
            // Another pointer exists *or* this value is borrowed; clone
            *this = ArcRef::new((**this).clone());
        }

        unsafe {
            // This unsafety is ok because we're guaranteed that the pointer
            // returned is the *only* pointer that will ever be returned to T. Our
            // reference count is guaranteed to be 1 at this point, and we required
            // the Arc itself to be `mut`, so we're returning the only possible
            // reference to the inner data.
            &mut *this.ptr()
        }
    }

    /// Provides mutable access to the contents _if_ the [`ArcRef`] is uniquely owned.
    #[inline]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if Self::is_unique(this) {
            unsafe {
                // See make_mut() for documentation of the threadsafety here.
                Some(&mut *this.ptr())
            }
        } else {
            None
        }
    }

    /// Whether or not the [`ArcRef`] is uniquely owned (is the refcount 1, and is `ArcBorrow` itself owned?).
    #[inline]
    pub fn is_unique(this: &Self) -> bool {
        // See the extensive discussion in [1] for why this needs to be Acquire.
        //
        // [1] https://github.com/servo/servo/issues/21186
        ArcRef::is_owned(this) && Self::count(this) == 1
    }

    /// Gets the number of [`Arc`] pointers to this allocation
    #[inline]
    pub fn count(this: &Self) -> usize {
        Self::load_count(this, atomic::Ordering::Acquire)
    }

    /// Gets the number of [`Arc`] pointers to this allocation, with a given load ordering
    #[inline]
    pub fn load_count(this: &Self, order: atomic::Ordering) -> usize {
        unsafe { (*ArcInner::count_ptr(this.ptr())).load(order) }
    }

    /// Returns an [`ArcBox`] if the [`ArcRef`] has exactly one strong, owned reference.
    ///
    /// Otherwise, an [`Err`] is returned with the same [`ArcRef`] that was
    /// passed in.
    ///
    /// # Examples
    ///
    /// ```
    /// use elysees::{ArcRef, ArcBox};
    ///
    /// let x = ArcRef::new(3);
    /// assert_eq!(ArcBox::into_inner(ArcRef::try_unique(x).unwrap()), 3);
    ///
    /// let x = ArcRef::new(4);
    /// let _y = ArcRef::clone(&x);
    /// assert_eq!(
    ///     *ArcRef::try_unique(x).map(ArcBox::into_inner).unwrap_err(),
    ///     4,
    /// );
    /// ```
    #[inline]
    pub fn try_unique(this: Self) -> Result<ArcBox<T>, Self> {
        if ArcRef::is_unique(&this) {
            // Safety: The current arc is unique and making a `ArcBox`
            //         from it is sound
            unsafe { Ok(ArcBox::from_arc(Arc::from_raw(ArcRef::into_raw(this)))) }
        } else {
            Err(this)
        }
    }

    /// Construct an [`ArcRef<'a, T>`] from an [`Arc<T>`]
    ///
    /// # Examples
    ///
    /// ```rust
    /// use elysees::{Arc, ArcRef};
    ///
    /// let x = Arc::new(3);
    /// let y = ArcRef::from_arc(x.clone());
    /// assert_eq!(ArcRef::count(&y), 2);
    /// ```
    #[inline]
    pub fn from_arc(arc: Arc<T>) -> Self {
        unsafe { Self::from_raw(Arc::into_raw(arc), true) }
    }

    /// Construct an `ArcRef<'a, T>` from an `ArcBorrow<'a, T>`
    #[inline]
    pub fn from_borrow(arc: ArcBorrow<'a, T>) -> Self {
        unsafe { Self::from_raw(arc.p.as_ptr(), false) }
    }

    /// Try to convert this `ArcRef<'a, T>` into an `Arc<T>` if owned; otherwise, return it as an `ArcBorrow`
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(3);
    /// assert_eq!(*ArcRef::try_into_arc(x.clone()).unwrap(), 3);
    /// ```
    #[inline]
    pub fn try_into_arc(this: Self) -> Result<Arc<T>, ArcBorrow<'a, T>> {
        match this.into_raw_inner() {
            (p, true) => Ok(unsafe { Arc::from_raw(p.as_ptr()) }),
            (p, false) => Err(ArcBorrow {
                p,
                phantom: PhantomData,
            }),
        }
    }

    /// Transform an [`ArcRef`] into an allocated [`ArcInner`] and ownership count.
    #[inline]
    pub(crate) fn into_raw_inner(self) -> (NonNull<T>, bool) {
        let p = self.nn_ptr();
        let o = ArcRef::is_owned(&self);
        core::mem::forget(self);
        (p, o)
    }

    /// Construct an [`ArcRef`] from an allocated [`ArcInner`] and ownership count.
    /// # Safety
    /// The `ptr` must point to a valid instance, allocated by an [`Arc`]. The reference count will
    /// not be modified.
    #[inline]
    pub(crate) unsafe fn from_raw(p: *const T, o: bool) -> Self {
        //TODO: replace with ptr_union...
        let result = ArcRef {
            p: Erasable::erase(NonNull::new_unchecked(
                (Erasable::erase(NonNull::new_unchecked(p as *mut T))
                    .as_ptr()
                    .wrapping_byte_add(if o { 0b10 } else { 0b00 })) as *mut u8,
            )),
            phantom: PhantomData,
        };
        debug_assert_eq!(ArcRef::is_owned(&result), o);
        result
    }

    /// Test pointer equality between the two [`ArcRef`]s, i.e. they must be the _same_
    /// allocation
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.nn_ptr() == other.nn_ptr()
    }

    #[inline]
    pub(crate) fn nn_ptr(&self) -> NonNull<T> {
        let buf_ptr = self
            .p
            .as_ptr()
            .wrapping_byte_sub(self.p.as_ptr() as usize & 0b11);
        let erased = unsafe { Erasable::erase(NonNull::new_unchecked(buf_ptr)) };
        unsafe { Erasable::unerase(erased) }
    }

    #[inline]
    pub(crate) fn ptr(&self) -> *mut T {
        self.nn_ptr().as_ptr()
    }

    /// Leak this [`ArcRef`], getting an [`ArcBorrow<'static, T>`]
    ///
    /// You can call the [`get`][`ArcBorrow::get`] method on the returned [`ArcBorrow`] to get an `&'static T`.
    /// Note that using this can (obviously) cause memory leaks!
    #[inline]
    pub fn leak(this: ArcRef<T>) -> ArcBorrow<'static, T> {
        let result = ArcBorrow {
            p: this.nn_ptr(),
            phantom: PhantomData,
        };
        mem::forget(ArcRef::into_owned(this));
        result
    }

    /// Get whether this [`ArcRef`] is owned
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(3);
    /// assert!(ArcRef::is_owned(&x));
    /// let y = x.clone();
    /// assert!(ArcRef::is_owned(&y));
    /// let z = ArcRef::into_borrow(&x);
    /// assert!(!ArcRef::is_owned(&z));
    /// ```
    #[inline]
    pub fn is_owned(this: &Self) -> bool {
        this.p.as_ptr() as usize & 0b10 != 0
    }

    /// Borrow this as an [`ArcBorrow`]. This does *not* bump the refcount.
    ///
    /// # Examples
    /// ```rust
    /// use elysees::{ArcBorrow, ArcRef};
    ///
    /// let x: ArcRef<u64> = ArcRef::new(3);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let y: ArcBorrow<u64> = ArcRef::borrow_arc(&x);
    /// assert_eq!(ArcRef::as_ptr(&x), ArcBorrow::into_raw(y));
    /// assert_eq!(ArcRef::count(&x), 1);
    /// assert_eq!(ArcBorrow::count(y), 1);
    /// ```
    #[inline]
    pub fn borrow_arc(this: &'a Self) -> ArcBorrow<'a, T> {
        ArcBorrow {
            p: this.nn_ptr(),
            phantom: PhantomData,
        }
    }

    /// Get this as an [`Arc`], bumping the refcount if necessary.
    ///
    /// # Examples
    /// ```rust
    /// use elysees::{Arc, ArcRef};
    ///
    /// let x = ArcRef::new(3);
    /// let y = ArcRef::into_borrow(&x);
    /// assert_eq!(ArcRef::as_ptr(&x), ArcRef::as_ptr(&y));
    /// assert_eq!(ArcRef::count(&x), 1);
    /// assert_eq!(ArcRef::count(&y), 1);
    /// let z = ArcRef::into_arc(y);
    /// assert_eq!(ArcRef::as_ptr(&x), Arc::as_ptr(&z));
    /// assert_eq!(ArcRef::count(&x), 2);
    /// assert_eq!(Arc::count(&z), 2);
    /// let w = ArcRef::into_arc(x);
    /// assert_eq!(Arc::count(&w), 2);
    /// assert_eq!(Arc::count(&z), 2);
    /// ```
    #[inline]
    pub fn into_arc(this: ArcRef<'a, T>) -> Arc<T> {
        match ArcRef::try_into_arc(this) {
            Ok(arc) => arc,
            Err(borrow) => ArcBorrow::clone_arc(borrow),
        }
    }

    /// Clone this as an [`Arc`].
    ///
    /// # Examples
    /// ```rust
    /// use elysees::{Arc, ArcRef};
    ///
    /// let x: ArcRef<u64> = ArcRef::new(3);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let y: Arc<u64> = ArcRef::clone_arc(&x);
    /// assert_eq!(ArcRef::as_ptr(&x), Arc::as_ptr(&y));
    /// assert_eq!(ArcRef::count(&x), 2);
    /// assert_eq!(Arc::count(&y), 2);
    /// ```
    #[inline]
    pub fn clone_arc(this: &'a Self) -> Arc<T> {
        ArcBorrow::clone_arc(ArcRef::borrow_arc(this))
    }

    /// Get this as an owned [`ArcRef`], with the `'static` lifetime
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(7);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let y = ArcRef::into_borrow(&x);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// assert_eq!(ArcRef::count(&y), 1);
    /// let z = ArcRef::into_owned(y);
    /// assert_eq!(ArcRef::as_ptr(&x), ArcRef::as_ptr(&z));
    /// assert_eq!(ArcRef::count(&x), 2);
    /// assert_eq!(ArcRef::count(&z), 2);
    /// ```
    #[inline]
    pub fn into_owned(this: Self) -> ArcRef<'static, T> {
        match Self::try_into_arc(this) {
            Ok(arc) => ArcRef::from_arc(arc),
            Err(borrow) => ArcRef::from_arc(ArcBorrow::clone_arc(borrow)),
        }
    }

    /// Borrow this as an [`ArcRef`]. This does *not* bump the refcount.
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(8);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let y = ArcRef::into_borrow(&x);
    /// assert_eq!(ArcRef::as_ptr(&x), ArcRef::as_ptr(&y));
    /// assert_eq!(ArcRef::count(&x), 1);
    /// assert_eq!(ArcRef::count(&y), 1);
    /// ```
    #[inline]
    pub fn into_borrow(this: &'a ArcRef<'a, T>) -> ArcRef<'a, T> {
        ArcRef::from_borrow(ArcRef::borrow_arc(this))
    }

    /// Clone this into an owned [`ArcRef`], with the `'static` lifetime
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    ///
    /// let x = ArcRef::new(7);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let y = ArcRef::into_borrow(&x);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// assert_eq!(ArcRef::count(&y), 1);
    /// let z = ArcRef::clone_into_owned(&y);
    /// assert_eq!(ArcRef::as_ptr(&x), ArcRef::as_ptr(&z));
    /// assert_eq!(ArcRef::count(&x), 2);
    /// assert_eq!(ArcRef::count(&y), 2);
    /// assert_eq!(ArcRef::count(&z), 2);
    /// ```
    #[inline]
    pub fn clone_into_owned(this: &Self) -> ArcRef<'static, T> {
        ArcRef::from_arc(ArcRef::clone_arc(this))
    }

    /// Get the internal pointer of an [`ArcBorrow`]. This does *not* bump the refcount.
    ///
    /// # Examples
    /// ```rust
    /// use elysees::{Arc, ArcRef};
    ///
    /// let x = ArcRef::new(7);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let x_ = x.clone();
    /// assert_eq!(ArcRef::count(&x), 2);
    /// let p = ArcRef::into_raw(x_);
    /// assert_eq!(ArcRef::count(&x), 2);
    /// assert_eq!(ArcRef::as_ptr(&x), p);
    /// let y = unsafe { Arc::from_raw(p) };
    /// assert_eq!(ArcRef::as_ptr(&x), Arc::as_ptr(&y));
    /// assert_eq!(ArcRef::count(&x), 2);
    /// std::mem::drop(y);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// ```
    #[inline]
    pub fn into_raw(this: Self) -> *const T {
        let result = ArcBorrow::into_raw(ArcRef::borrow_arc(&this));
        mem::forget(this);
        result
    }

    /// Get the internal pointer of an [`ArcBorrow`]. This does *not* bump the refcount.
    ///
    /// # Examples
    /// ```rust
    /// use elysees::ArcRef;
    /// let x = ArcRef::new(7);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// let p = ArcRef::as_ptr(&x);
    /// assert_eq!(ArcRef::count(&x), 1);
    /// ```
    #[inline]
    pub fn as_ptr(this: &Self) -> *const T {
        ArcBorrow::into_raw(ArcRef::borrow_arc(this))
    }
}

impl<'a, T: Erasable> Drop for ArcRef<'a, T> {
    #[inline]
    fn drop(&mut self) {
        if ArcRef::is_owned(self) {
            core::mem::drop(unsafe { Arc::from_raw(self.ptr()) })
        }
    }
}

impl<'a, T: Erasable> Clone for ArcRef<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        if ArcRef::is_owned(self) {
            ArcRef::from_arc(ArcRef::clone_arc(self))
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
        unsafe { &*self.ptr() }
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

impl<'a, T: Erasable> fmt::Pointer for ArcRef<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.nn_ptr(), f)
    }
}

impl<'a, T: Erasable + Default> Default for ArcRef<'a, T> {
    #[inline]
    fn default() -> ArcRef<'a, T> {
        let d = ArcRef::new(Default::default());
        d
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
        self
    }
}

impl<'a, T: Erasable> AsRef<T> for ArcRef<'a, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
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
