use alloc::alloc::alloc;
use core::alloc::Layout;
use core::borrow;
use core::cmp::Ordering;
use core::convert::From;
use core::ffi::c_void;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::Deref;
use core::ptr;
use core::sync::atomic;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::{isize, usize};
use erasable::{Erasable, ErasablePtr};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "slice-dst")]
use slice_dst::{AllocSliceDst, SliceDst, TryAllocSliceDst};
#[cfg(feature = "stable_deref_trait")]
use stable_deref_trait::{CloneStableDeref, StableDeref};

use crate::{abort, ArcBorrow, ArcBox};

/// A soft limit on the amount of references that may be made to an `Arc`.
///
/// Going above this limit will abort your program (although not
/// necessarily) at _exactly_ `MAX_REFCOUNT + 1` references.
const MAX_REFCOUNT: usize = (isize::MAX) as usize;

/// The object allocated by an Arc<T>
#[repr(C)]
pub struct ArcInner<T: ?Sized> {
    pub(crate) count: atomic::AtomicUsize,
    pub(crate) data: T,
}

impl<T> ArcInner<T> {
    /// Get the offset of the data pointer from the beginning of the inner pointer
    #[inline]
    pub fn data_offset() -> usize {
        Layout::new::<atomic::AtomicUsize>()
            .extend(Layout::new::<T>())
            .unwrap()
            .1
    }

    /// Given the inner pointer, get a data pointer
    ///
    /// # Safety:
    /// This must be a pointer to a (potentially uninitialized) `ArcInner`
    #[inline]
    pub unsafe fn data_ptr(this: *mut ArcInner<T>) -> *mut T {
        let ptr = (this as *mut u8).add(Self::data_offset()) as *mut _;
        debug_assert_eq!(ArcInner::from_data(ptr), this);
        ptr
    }

    /// Given a data pointer, get the inner pointer
    ///
    /// # Safety:
    /// This must be a pointer to the `data` field of a (potentially uninitialized) `ArcInner` with pointer provenance consisting of the entire `ArcInner`
    #[inline]
    pub unsafe fn from_data(data: *mut T) -> *mut ArcInner<T> {
        let ptr = (data as *mut u8).sub(Self::data_offset()) as *mut _;
        ptr
    }
}

impl<T: ?Sized> ArcInner<T> {
    /// Get the layout of an `ArcInner<T>` given the data, along with the data offset
    #[inline]
    pub fn layout(data: &T) -> (Layout, usize) {
        let (unpadded_layout, data_offset) = Layout::new::<atomic::AtomicUsize>()
            .extend(Layout::for_value(data))
            .unwrap();
        (unpadded_layout.pad_to_align(), data_offset)
    }

    /// Get the offset of the data pointer from the beginning of the inner pointer, given the data
    #[inline]
    pub fn data_offset_value(data: &T) -> usize {
        Self::layout(data).1
    }

    /// Given a data pointer, get the count pointer
    ///
    /// # Safety:
    /// This must be a pointer to the `data` field of an initialized `ArcInner` with pointer provenance consisting of the entire `ArcInner`
    #[inline]
    pub unsafe fn count_ptr(data: *mut T) -> *mut atomic::AtomicUsize {
        (data as *mut u8).sub(Self::data_offset_value(&*data)) as *mut _
    }
}

unsafe impl<T: ?Sized + Sync + Send> Send for ArcInner<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for ArcInner<T> {}

/// An atomically reference counted shared pointer
///
/// See the documentation for [`Arc`][aa] in the standard library. Unlike the
/// standard library [`Arc`][aa], this [`Arc`] does not support weak reference counting.
///
/// [aa]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html
#[repr(transparent)]
pub struct Arc<T: ?Sized> {
    pub(crate) p: ptr::NonNull<T>,
    phantom: PhantomData<T>,
}

unsafe impl<T: ?Sized + Sync + Send> Send for Arc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Arc<T> {}

impl<T> Arc<T> {
    /// Construct an [`Arc`]
    #[inline]
    pub fn new(data: T) -> Self {
        let (layout, _offset) = ArcInner::layout(&data);
        let result = unsafe {
            let p = alloc::alloc::alloc(layout) as *mut ArcInner<T>;
            p.write(ArcInner {
                count: atomic::AtomicUsize::new(1),
                data,
            });
            Arc::from_raw_inner(ptr::NonNull::new_unchecked(p))
        };
        result
    }

    /// Transform an [`Arc`] into an allocated [`ArcInner`].
    #[inline]
    pub(crate) fn into_raw_inner(self) -> ptr::NonNull<ArcInner<T>> {
        let p = self.p.as_ptr();
        core::mem::forget(self);
        unsafe { ptr::NonNull::new_unchecked(ArcInner::from_data(p)) }
    }

    /// Construct an [`Arc`] from an allocated [`ArcInner`].
    /// # Safety
    /// The `ptr` must point to a valid instance, allocated by an [`Arc`]. The reference count will
    /// not be modified.
    #[inline]
    pub(crate) unsafe fn from_raw_inner(p: ptr::NonNull<ArcInner<T>>) -> Self {
        Arc {
            p: ptr::NonNull::new_unchecked(ArcInner::data_ptr(p.as_ptr())),
            phantom: PhantomData,
        }
    }

    /// Returns the inner value, if the [`Arc`] has exactly one strong reference.
    ///
    /// Otherwise, an [`Err`] is returned with the same [`Arc`] that was
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
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        Self::try_unique(this).map(ArcBox::into_inner)
    }
}

impl<T: ?Sized> Arc<T> {
    /// Reconstruct the [`Arc<T>`][`Arc`] from a raw pointer obtained from [`into_raw`][`Arc::into_raw`]
    ///
    /// Note: This raw pointer will be offset in the allocation and must be preceded
    /// by the atomic count.
    ///
    /// It is recommended to use [`OffsetArc`] for this
    #[inline]
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Arc {
            p: ptr::NonNull::new_unchecked(ptr as *mut T),
            phantom: PhantomData,
        }
    }

    /// Convert the [`Arc`] to a raw pointer, suitable for use across FFI
    ///
    /// Note: This returns a pointer to the data `T`, which is offset in the allocation.
    ///
    /// It is recommended to use [`OffsetArc`] for this.
    #[inline]
    pub fn into_raw(this: Self) -> *const T {
        let ptr = Arc::as_ptr(&this);
        mem::forget(this);
        ptr
    }

    /// Returns the raw pointer.
    ///
    /// Same as into_raw except `self` isn't consumed.
    #[inline]
    pub fn as_ptr(this: &Arc<T>) -> *const T {
        this.p.as_ptr()
    }

    /// Produce a pointer to the data that can be converted back
    /// to an Arc. This is basically an [`&Arc<T>`][`Arc`], without the extra indirection.
    /// It has the benefits of an `&T` but also knows about the underlying refcount
    /// and can be converted into more [`Arc<T>`][`Arc`]s if necessary.
    #[inline]
    pub fn borrow_arc(this: &Self) -> ArcBorrow<'_, T> {
        ArcBorrow {
            p: this.p,
            phantom: PhantomData,
        }
    }

    /// Returns the address on the heap of the [`Arc`] itself -- not the `T` within it -- for memory
    /// reporting.
    pub fn heap_ptr(&self) -> *const c_void {
        unsafe { ArcInner::count_ptr(self.p.as_ptr()) as *const c_void }
    }

    // Non-inlined part of [`drop`][`Arc::drop`]. Just invokes the destructor.
    #[inline(never)]
    pub(crate) unsafe fn drop_slow(&mut self) {
        let (layout, data_offset) = ArcInner::layout(&**self);
        alloc::alloc::dealloc((self.p.as_ptr() as *mut u8).sub(data_offset), layout)
    }

    /// Test pointer equality between the two [`Arc`]s, i.e. they must be the _same_
    /// allocation
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.p == other.p
    }

    /// Leak this [`Arc<T>`][`Arc`], getting an [`ArcBorrow<'static, T>`][`ArcBorrow`]
    ///
    /// You can call the [`get`][`ArcBorrow::get`] method on the returned [`ArcBorrow`] to get an `&'static T`.
    /// Note that using this can (obviously) cause memory leaks!
    #[inline]
    pub fn leak(this: Arc<T>) -> ArcBorrow<'static, T> {
        let result = ArcBorrow {
            p: this.p,
            phantom: PhantomData,
        };
        mem::forget(this);
        result
    }
}

impl<T> Arc<MaybeUninit<T>> {
    /// Create an [`Arc`] containing a [`MaybeUninit<T>`][`core::mem::MaybeUninit`].
    pub fn new_uninit() -> Self {
        Arc::new(MaybeUninit::<T>::uninit())
    }

    /// Calls `MaybeUninit::write` on the value contained.
    pub fn write(&mut self, val: T) -> &mut T {
        unsafe {
            self.p.as_ptr().write(MaybeUninit::new(val));
            &mut *self.p.as_mut().as_mut_ptr()
        }
    }

    /// Obtain a mutable pointer to the stored `MaybeUninit<T>`.
    pub fn as_mut_ptr(&mut self) -> *mut MaybeUninit<T> {
        self.p.as_ptr()
    }

    /// # Safety
    ///
    /// Must initialize all fields before calling this function.
    #[inline]
    pub unsafe fn assume_init(self) -> Arc<T> {
        Arc::from_raw(Arc::into_raw(self) as *const T)
    }
}

impl<T> Arc<[MaybeUninit<T>]> {
    /// Create an [`Arc`] contains an array `[MaybeUninit<T>]` of `len`.
    pub fn new_uninit_slice(len: usize) -> Self {
        // layout should work as expected since ArcInner uses C representation.
        let layout = Layout::new::<atomic::AtomicUsize>();
        let array_layout = Layout::array::<MaybeUninit<T>>(len).unwrap();

        let (layout, offset) = layout.extend(array_layout).unwrap();
        let layout = layout.pad_to_align();

        // Allocate and initialize ArcInner
        unsafe {
            let ptr = alloc(layout);
            (ptr as *mut atomic::AtomicUsize).write(atomic::AtomicUsize::new(1));
            let slice = ptr::slice_from_raw_parts_mut(ptr.add(offset) as *mut MaybeUninit<T>, len);
            let result = Arc::from_raw(slice);
            result
        }
    }

    /// # Safety
    ///
    /// Must initialize all fields before calling this function.
    #[inline]
    pub unsafe fn assume_init(self) -> Arc<[T]> {
        Arc::from_raw(Arc::into_raw(self) as *const [T])
    }
}

impl<T: ?Sized> Clone for Arc<T> {
    #[inline]
    fn clone(&self) -> Self {
        // Using a relaxed ordering is alright here, as knowledge of the
        // original reference prevents other threads from erroneously deleting
        // the object.
        //
        // As explained in the [Boost documentation][1], Increasing the
        // reference counter can always be done with memory_order_relaxed: New
        // references to an object can only be formed from an existing
        // reference, and passing an existing reference from one thread to
        // another must already provide any required synchronization.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        let old_size = unsafe { (*ArcInner::count_ptr(self.p.as_ptr())).fetch_add(1, Relaxed) };

        // However we need to guard against massive refcounts in case someone
        // is `mem::forget`ing Arcs. If we don't do this the count can overflow
        // and users will use-after free. We racily saturate to `isize::MAX` on
        // the assumption that there aren't ~2 billion threads incrementing
        // the reference count at once. This branch will never be taken in
        // any realistic program.
        //
        // We abort because such a program is incredibly degenerate, and we
        // don't care to support it.
        if old_size > MAX_REFCOUNT {
            abort();
        }

        Arc {
            p: self.p,
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> Deref for Arc<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        unsafe { self.p.as_ref() }
    }
}

impl<T: Clone + ?Sized> Arc<T> {
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
    pub fn make_mut(this: &mut Self) -> &mut T {
        if !Self::is_unique(this) {
            // Another pointer exists; clone
            *this = Arc::new((**this).clone());
        }
        debug_assert!(Self::is_unique(this));

        unsafe {
            // This unsafety is ok because we're guaranteed that the pointer
            // returned is the *only* pointer that will ever be returned to T. Our
            // reference count is guaranteed to be 1 at this point, and we required
            // the Arc itself to be `mut`, so we're returning the only possible
            // reference to the inner data.
            this.p.as_mut()
        }
    }
}

impl<T: ?Sized> Arc<T> {
    /// Provides mutable access to the contents _if_ the [`Arc`] is uniquely owned.
    #[inline]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if Self::is_unique(this) {
            unsafe {
                // See make_mut() for documentation of the threadsafety here.
                Some(this.p.as_mut())
            }
        } else {
            None
        }
    }

    /// Whether or not the [`Arc`] is uniquely owned (is the refcount 1?).
    #[inline]
    pub fn is_unique(this: &Self) -> bool {
        // See the extensive discussion in [1] for why this needs to be Acquire.
        //
        // [1] https://github.com/servo/servo/issues/21186
        let u = Self::count(this) == 1;
        u
    }

    /// Gets the number of [`Arc`] pointers to this allocation
    #[inline]
    pub fn count(this: &Self) -> usize {
        Self::load_count(this, atomic::Ordering::Acquire)
    }

    /// Gets the number of [`Arc`] pointers to this allocation, with a given load ordering
    #[inline]
    pub fn load_count(this: &Self, order: atomic::Ordering) -> usize {
        unsafe { (*ArcInner::count_ptr(this.p.as_ptr())).load(order) }
    }

    /// Returns an [`ArcBox`] if the [`Arc`] has exactly one strong reference.
    ///
    /// Otherwise, an [`Err`] is returned with the same [`Arc`] that was
    /// passed in.
    ///
    /// # Examples
    ///
    /// ```
    /// use elysees::{Arc, ArcBox};
    ///
    /// let x = Arc::new(3);
    /// assert_eq!(ArcBox::into_inner(Arc::try_unique(x).unwrap()), 3);
    ///
    /// let x = Arc::new(4);
    /// let _y = Arc::clone(&x);
    /// assert_eq!(
    ///     *Arc::try_unique(x).map(ArcBox::into_inner).unwrap_err(),
    ///     4,
    /// );
    /// ```
    #[inline]
    pub fn try_unique(this: Self) -> Result<ArcBox<T>, Self> {
        if Self::is_unique(&this) {
            // Safety: The current arc is unique and making a `ArcBox`
            //         from it is sound
            unsafe { Ok(ArcBox::from_arc(this)) }
        } else {
            Err(this)
        }
    }

    /// Convert this [`Arc`] to an [`ArcBox`], cloning the internal data if necessary for uniqueness
    #[inline]
    pub fn unique(this: Self) -> ArcBox<T>
    where
        T: Clone,
    {
        if Self::is_unique(&this) {
            ArcBox(this)
        } else {
            ArcBox::new(this.deref().clone())
        }
    }
}

impl<T: ?Sized> Drop for Arc<T> {
    #[inline]
    fn drop(&mut self) {
        // Because `fetch_sub` is already atomic, we do not need to synchronize
        // with other threads unless we are going to delete the object.
        if unsafe { (*ArcInner::count_ptr(self.p.as_ptr())).fetch_sub(1, Release) != 1 } {
            return;
        }

        // FIXME(bholley): Use the updated comment when [2] is merged.
        //
        // This load is needed to prevent reordering of use of the data and
        // deletion of the data.  Because it is marked `Release`, the decreasing
        // of the reference count synchronizes with this `Acquire` load. This
        // means that use of the data happens before decreasing the reference
        // count, which happens before this load, which happens before the
        // deletion of the data.
        //
        // As explained in the [Boost documentation][1],
        //
        // > It is important to enforce any possible access to the object in one
        // > thread (through an existing reference) to *happen before* deleting
        // > the object in a different thread. This is achieved by a "release"
        // > operation after dropping a reference (any access to the object
        // > through this reference must obviously happened before), and an
        // > "acquire" operation before deleting the object.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        // [2]: https://github.com/rust-lang/rust/pull/41714
        unsafe { (*ArcInner::count_ptr(self.p.as_ptr())).load(Acquire) };

        unsafe {
            self.drop_slow();
        }
    }
}

impl<T: ?Sized, U: ?Sized + PartialEq<T>> PartialEq<Arc<T>> for Arc<U> {
    #[inline]
    fn eq(&self, other: &Arc<T>) -> bool {
        *(*self) == *(*other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    #[inline]
    fn ne(&self, other: &Arc<T>) -> bool {
        *(*self) != *(*other)
    }
}

impl<T: ?Sized, U: ?Sized + PartialOrd<T>> PartialOrd<Arc<T>> for Arc<U> {
    #[inline]
    fn partial_cmp(&self, other: &Arc<T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }

    #[inline]
    fn lt(&self, other: &Arc<T>) -> bool {
        *(*self) < *(*other)
    }

    #[inline]
    fn le(&self, other: &Arc<T>) -> bool {
        *(*self) <= *(*other)
    }

    #[inline]
    fn gt(&self, other: &Arc<T>) -> bool {
        *(*self) > *(*other)
    }

    #[inline]
    fn ge(&self, other: &Arc<T>) -> bool {
        *(*self) >= *(*other)
    }
}

impl<T: ?Sized + Ord> Ord for Arc<T> {
    fn cmp(&self, other: &Arc<T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + Eq> Eq for Arc<T> {}

impl<T: ?Sized + fmt::Display> fmt::Display for Arc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Arc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized> fmt::Pointer for Arc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.p, f)
    }
}

impl<T: Default> Default for Arc<T> {
    #[inline]
    fn default() -> Arc<T> {
        let d = Arc::new(Default::default());
        d
    }
}

impl<T: ?Sized + Hash> Hash for Arc<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<T> From<T> for Arc<T> {
    #[inline]
    fn from(t: T) -> Self {
        Arc::new(t)
    }
}

impl<T: ?Sized> borrow::Borrow<T> for Arc<T> {
    #[inline]
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T: ?Sized> AsRef<T> for Arc<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        &**self
    }
}

unsafe impl<T: ?Sized + Erasable> ErasablePtr for Arc<T> {
    #[inline]
    fn erase(this: Self) -> erasable::ErasedPtr {
        T::erase(unsafe { ptr::NonNull::new_unchecked(Arc::into_raw(this) as *mut _) })
    }

    #[inline]
    unsafe fn unerase(this: erasable::ErasedPtr) -> Self {
        Arc::from_raw(T::unerase(this).as_ptr())
    }
}

#[cfg(feature = "stable_deref_trait")]
unsafe impl<T: ?Sized> StableDeref for Arc<T> {}
#[cfg(feature = "stable_deref_trait")]
unsafe impl<T: ?Sized> CloneStableDeref for Arc<T> {}

#[cfg(feature = "serde")]
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Arc<T> {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Arc<T>, D::Error>
    where
        D: ::serde::de::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Arc::new)
    }
}

#[cfg(feature = "serde")]
impl<T: Serialize> Serialize for Arc<T> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ::serde::ser::Serializer,
    {
        (**self).serialize(serializer)
    }
}

#[cfg(feature = "unsize")]
/// # Safety
///
/// This implementation must guarantee that it is sound to call replace_ptr with an unsized variant
/// of the pointer retuned in `as_sized_ptr`. The basic property of Unsize coercion is that safety
/// variants and layout is unaffected. The Arc does not rely on any other property of T. This makes
/// any unsized ArcInner valid for being shared with the sized variant.
/// This does _not_ mean that any T can be unsized into an U, but rather than if such unsizing is
/// possible then it can be propagated into the Arc<T>.
unsafe impl<T, U: ?Sized> unsize::CoerciblePtr<U> for Arc<T> {
    type Pointee = T;
    type Output = Arc<U>;

    fn as_sized_ptr(&mut self) -> *mut T {
        // Returns a pointer to the complete inner. The unsizing itself won't care about the
        // pointer value and promises not to offset it.
        self.p.as_ptr()
    }

    unsafe fn replace_ptr(self, new: *mut U) -> Arc<U> {
        // Fix the provenance by ensuring that of `self` is used.
        let old_layout = ArcInner::layout(&*self);
        let inner = ManuallyDrop::new(self);
        let p = inner.p.as_ptr() as *mut T;
        // Safety: The caller upholds that `new` is an unsized version of the data in the previous ArcInner.
        let result = Arc::from_raw(p.replace_ptr(new) as *mut U);
        debug_assert_eq!(old_layout, ArcInner::layout(&*result));
        result
    }
}

#[cfg(feature = "slice-dst")]
/// # Safety
///
/// `ArcInner<S>` is implemented as an additional header before `S`, consisting of the reference count
unsafe impl<S: SliceDst + ?Sized> SliceDst for ArcInner<S> {
    fn layout_for(len: usize) -> Layout {
        Layout::new::<atomic::AtomicUsize>()
            .extend(S::layout_for(len))
            .unwrap()
            .0
            .pad_to_align()
    }

    fn retype(ptr: ptr::NonNull<[()]>) -> ptr::NonNull<Self> {
        let retype_inner = S::retype(ptr);
        // Safety: the metadata for `S` is the same as for `ArcInner<S>`, since `ArcInner<S>` has `S` as it's last member.
        // This is based on the implementation for `triomphe`.
        let retyped = unsafe { core::ptr::NonNull::new_unchecked(retype_inner.as_ptr() as *mut _) };
        //TODO: add correctness assertions
        retyped
    }
}

#[cfg(feature = "slice-dst")]
/// # Safety
///
/// This function merely delegates to the [`TryAllocSliceDst`] implementation
unsafe impl<S: ?Sized + SliceDst> AllocSliceDst<S> for Arc<S> {
    unsafe fn new_slice_dst<I>(len: usize, init: I) -> Self
    where
        I: FnOnce(ptr::NonNull<S>),
    {
        #[allow(clippy::unit_arg)]
        let init = |ptr| Ok::<(), core::convert::Infallible>(init(ptr));
        match Self::try_new_slice_dst(len, init) {
            Ok(a) => a,
            Err(void) => match void {},
        }
    }
}

#[cfg(feature = "slice-dst")]
/// # Safety
///
/// This function merely delegates to the [`ArcBox`] implementation
unsafe impl<S: ?Sized + SliceDst> TryAllocSliceDst<S> for Arc<S> {
    unsafe fn try_new_slice_dst<I, E>(len: usize, init: I) -> Result<Self, E>
    where
        I: FnOnce(ptr::NonNull<S>) -> Result<(), E>,
    {
        Ok(ArcBox::try_new_slice_dst(len, init)?.shareable())
    }
}

#[cfg(test)]
mod tests {
    use crate::arc::Arc;
    use core::mem::MaybeUninit;
    #[cfg(feature = "unsize")]
    use unsize::{CoerceUnsize, Coercion};

    #[test]
    fn try_unwrap() {
        let x = Arc::new(100usize);
        let y = x.clone();

        // The count should be two so `try_unwrap()` should fail
        assert_eq!(Arc::count(&x), 2);
        assert!(Arc::try_unwrap(x).is_err());

        // Since `x` has now been dropped, the count should be 1
        // and `try_unwrap()` should succeed
        assert_eq!(Arc::count(&y), 1);
        assert_eq!(Arc::try_unwrap(y), Ok(100));
    }

    #[test]
    #[cfg(feature = "unsize")]
    fn coerce_to_slice() {
        let x = Arc::new([0u8; 4]);
        let y: Arc<[u8]> = x.clone().unsize(Coercion::to_slice());
        assert_eq!((*x).as_ptr(), (*y).as_ptr());
    }

    #[test]
    #[cfg(feature = "unsize")]
    fn coerce_to_dyn() {
        use crate::ArcInner;

        let x: Arc<_> = Arc::new(|| 42u32);
        let old_layout = ArcInner::layout(&*x);
        assert_eq!((*x)(), 42);
        let x: Arc<_> = x.unsize(Coercion::<_, dyn Fn() -> u32>::to_fn());
        let new_layout = ArcInner::layout(&*x);
        assert_eq!(old_layout, new_layout);
        assert_eq!((*x)(), 42);
    }

    #[test]
    fn maybeuninit() {
        let mut arc: Arc<MaybeUninit<_>> = Arc::new_uninit();
        arc.write(999);

        let arc = unsafe { arc.assume_init() };
        assert_eq!(*arc, 999);
    }

    #[test]
    #[cfg(feature = "std")]
    fn maybeuninit_array() {
        let mut arc: Arc<[MaybeUninit<usize>]> = Arc::new_uninit_slice(5);
        assert!(Arc::is_unique(&arc));
        for (uninit, index) in Arc::get_mut(&mut arc).unwrap().iter_mut().zip(0..5) {
            let ptr = uninit.as_mut_ptr();
            unsafe { core::ptr::write(ptr, index) };
        }

        let arc = unsafe { arc.assume_init() };
        assert!(Arc::is_unique(&arc));
        // Using clone to that the layout generated in new_uninit_slice is compatible
        // with ArcInner.
        let arcs = [
            arc.clone(),
            arc.clone(),
            arc.clone(),
            arc.clone(),
            arc.clone(),
        ];
        assert_eq!(6, Arc::count(&arc));
        // If the layout is not compatible, then the data might be corrupted.
        assert_eq!(*arc, [0, 1, 2, 3, 4]);

        // Drop the arcs and check the count and the content to
        // make sure it isn't corrupted.
        drop(arcs);
        assert!(Arc::is_unique(&arc));
        assert_eq!(*arc, [0, 1, 2, 3, 4]);
    }

    #[test]
    #[cfg(feature = "slice-dst")]
    fn slice_with_header() {
        use slice_dst::SliceWithHeader;
        let slice = [0, 1, 2, 3, 4, 5];
        let arc: Arc<SliceWithHeader<u64, u32>> = SliceWithHeader::new(45, slice.iter().copied());
        let arc2 = SliceWithHeader::from_slice(45, &slice);
        assert_eq!(arc, arc2);
        assert_ne!(Arc::as_ptr(&arc), Arc::as_ptr(&arc2));
    }
}
