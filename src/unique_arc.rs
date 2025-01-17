use alloc::{alloc::Layout, boxed::Box};
use core::borrow::{Borrow, BorrowMut};
use core::convert::TryFrom;
use core::fmt::{self, Debug, Display, Formatter};
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::sync::atomic::AtomicUsize;

use super::{Arc, ArcInner, ArcRef};

#[cfg(feature = "slice-dst")]
use slice_dst::{AllocSliceDst, SliceDst, TryAllocSliceDst};

/// An [`Arc`] that is known to be uniquely owned
///
/// When [`Arc`]s are constructed, they are known to be
/// uniquely owned. In such a case it is safe to mutate
/// the contents of the [`Arc`]. Normally, one would just handle
/// this by mutating the data on the stack before allocating the
/// [`Arc`], however it's possible the data is large or unsized
/// and you need to heap-allocate it earlier in such a way
/// that it can be freely converted into a regular [`Arc`] once you're
/// done.
///
/// [`ArcBox`] exists for this purpose, when constructed it performs
/// the same allocations necessary for an [`Arc`], however it allows mutable access.
/// Once the mutation is finished, you can call [`.shareable()`](`ArcBox::shareable`) and get a regular [`Arc`]
/// out of it.
///
/// ```rust
/// # use elysees::ArcBox;
/// let data = [1, 2, 3, 4, 5];
/// let mut x = ArcBox::new(data);
/// x[4] = 7; // mutate!
/// let y = x.shareable(); // y is an Arc<T>
/// ```
#[repr(transparent)]
pub struct ArcBox<T: ?Sized>(pub(crate) Arc<T>);

impl<T> ArcBox<T> {
    #[inline]
    /// Construct a new [`ArcBox`]
    pub fn new(data: T) -> Self {
        ArcBox(Arc::new(data))
    }

    /// Construct an uninitialized [`ArcBox`]
    #[inline]
    pub fn new_uninit() -> ArcBox<MaybeUninit<T>> {
        unsafe {
            let layout = Layout::new::<ArcInner<MaybeUninit<T>>>();
            let ptr = alloc::alloc::alloc(layout);
            let mut p = NonNull::new(ptr)
                .unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout))
                .cast::<ArcInner<MaybeUninit<T>>>();
            ptr::write(&mut p.as_mut().count, AtomicUsize::new(1));

            ArcBox(Arc::from_raw_inner(p))
        }
    }

    /// Gets the inner value of this [`ArcBox`]
    pub fn into_inner(this: Self) -> T {
        // Wrap the Arc in a `ManuallyDrop` so that its drop routine never runs
        let this = ManuallyDrop::new(this.0);
        debug_assert!(
            Arc::is_unique(&this),
            "attempted to call `.into_inner()` on a `ArcBox` with a non-zero ref count",
        );

        // Safety: We have exclusive access to the inner data and the
        //         arc will not perform its drop routine since we've
        //         wrapped it in a `ManuallyDrop`
        unsafe { Box::from_raw(ArcInner::from_data(this.p.as_ptr())).data }
    }

    /// Convert to a shareable [`ArcRef<'static, T>`] once we're done mutating it
    #[inline]
    pub fn shareable_ref(self) -> ArcRef<'static, T> {
        ArcRef::from_arc(self.0)
    }
}

impl<T: ?Sized> ArcBox<T> {
    /// Convert to a shareable [`Arc<T>`] once we're done mutating it
    #[inline]
    pub fn shareable(self) -> Arc<T> {
        self.0
    }

    /// Creates a new [`ArcBox`] from the given [`Arc`].
    ///
    /// An unchecked alternative to [`Arc::try_unique`]
    ///
    /// # Safety
    ///
    /// The given [`Arc`] must have a reference count of exactly one
    ///
    pub(crate) unsafe fn from_arc(arc: Arc<T>) -> Self {
        debug_assert_eq!(Arc::count(&arc), 1);
        Self(arc)
    }
}

impl<T> ArcBox<MaybeUninit<T>> {
    /// Convert to an initialized [`Arc`].
    ///
    /// # Safety
    ///
    /// This function is equivalent to [`MaybeUninit::assume_init`] and has the
    /// same safety requirements. You are responsible for ensuring that the `T`
    /// has actually been initialized before calling this method.
    #[inline]
    pub unsafe fn assume_init(this: Self) -> ArcBox<T> {
        ArcBox(Arc::from_raw_inner(this.0.into_raw_inner().cast()))
    }
}

impl<T: ?Sized> TryFrom<Arc<T>> for ArcBox<T> {
    type Error = Arc<T>;

    fn try_from(arc: Arc<T>) -> Result<Self, Self::Error> {
        Arc::try_unique(arc)
    }
}

impl<T: ?Sized> Deref for ArcBox<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        #[allow(clippy::explicit_auto_deref)]
        &*self.0
    }
}

impl<T: ?Sized> DerefMut for ArcBox<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // We know this to be uniquely owned
        unsafe { self.0.p.as_mut() }
    }
}

impl<T: Clone> Clone for ArcBox<T> {
    #[inline]
    fn clone(&self) -> ArcBox<T> {
        ArcBox(Arc::new(self.0.deref().clone()))
    }
}

impl<T: Default> Default for ArcBox<T> {
    #[inline]
    fn default() -> ArcBox<T> {
        ArcBox::new(Default::default())
    }
}

impl<T: Debug> Debug for ArcBox<T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl<T: Display> Display for ArcBox<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<T: ?Sized> Borrow<T> for ArcBox<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for ArcBox<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> BorrowMut<T> for ArcBox<T> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsMut<T> for ArcBox<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

/// # Safety
/// This leverages the correctness of Arc's CoerciblePtr impl. Additionally, we must ensure that
/// this can not be used to violate the safety invariants of ArcBox, which require that we can not
/// duplicate the Arc, such that replace_ptr returns a valid instance. This holds since it consumes
/// a unique owner of the contained ArcInner.
#[cfg(feature = "unsize")]
unsafe impl<T, U: ?Sized> unsize::CoerciblePtr<U> for ArcBox<T> {
    type Pointee = T;
    type Output = ArcBox<U>;

    fn as_sized_ptr(&mut self) -> *mut T {
        // Dispatch to the contained field.
        unsize::CoerciblePtr::<U>::as_sized_ptr(&mut self.0)
    }

    unsafe fn replace_ptr(self, new: *mut U) -> ArcBox<U> {
        // Dispatch to the contained field, work around conflict of destructuring and Drop.
        let inner = ManuallyDrop::new(self);
        ArcBox(ptr::read(&inner.0).replace_ptr(new))
    }
}

/// This implementation is based on that in the [documentation for `slice-dst`](https://docs.rs/slice-dst/latest/slice_dst/trait.AllocSliceDst.html).
///
/// # Safety
///
/// This function merely calls `try_new_slice` with an initializer statically guaranteed never to fail, and therefore is safe if and only if
/// `try_new_slice` is.
#[cfg(feature = "slice-dst")]
unsafe impl<S: ?Sized + SliceDst> AllocSliceDst<S> for ArcBox<S> {
    unsafe fn new_slice_dst<I>(len: usize, init: I) -> Self
    where
        I: FnOnce(ptr::NonNull<S>),
    {
        #[allow(clippy::unit_arg)]
        let init = |ptr| Ok::<(), core::convert::Infallible>(init(ptr));
        #[allow(unreachable_patterns)]
        match Self::try_new_slice_dst(len, init) {
            Ok(a) => a,
            Err(void) => match void {},
        }
    }
}

#[cfg(feature = "slice-dst")]
/// # Safety
///
///
unsafe impl<S: ?Sized + SliceDst> TryAllocSliceDst<S> for ArcBox<S> {
    unsafe fn try_new_slice_dst<I, E>(len: usize, init: I) -> Result<Self, E>
    where
        I: FnOnce(ptr::NonNull<S>) -> Result<(), E>,
    {
        // Get the offset for the `S` field in an `ArcInner<S>`:
        let s_layout = S::layout_for(len);
        let (unpadded_layout, offset) = Layout::new::<AtomicUsize>().extend(s_layout).unwrap();

        // Get an allocation for an `ArcInner<S>`
        let ptr: NonNull<ArcInner<S>> = slice_dst::alloc_slice_dst(len);

        // Safety: Since this pointer is to the beginning of the allocation, we can initialize the counter through it...
        ptr.cast::<AtomicUsize>()
            .as_ptr()
            .write(AtomicUsize::new(1));

        // Safety: the offset `offset` is in bounds of the allocation `ptr`
        let s_data_ptr = ptr.cast::<u8>().as_ptr().add(offset) as *mut ();

        // Safety: we can construct `NonNull<[()]>` with length `len` and ptr `ptr`
        let s_slice_ptr: NonNull<[()]> =
            NonNull::new_unchecked(core::ptr::slice_from_raw_parts_mut(s_data_ptr, len));
        let s_ptr = S::retype(s_slice_ptr);

        match init(s_ptr) {
            Ok(()) => {
                // Yay! Everything was initialized! Do a few checks for good measure.
                debug_assert_eq!(Layout::for_value(&*s_ptr.as_ptr()), s_layout);
                let layout = unpadded_layout.pad_to_align();
                debug_assert_eq!(Layout::for_value(&*ptr.as_ptr()), layout);
            }
            Err(err) => {
                // Deallocate ptr and return an error
                let layout = unpadded_layout.pad_to_align();
                alloc::alloc::dealloc(ptr.as_ptr() as *mut u8, layout);
                return Err(err);
            }
        }

        Ok(ArcBox(Arc::from_raw(s_ptr.as_ptr())))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Arc, ArcBox};
    use core::convert::TryFrom;

    #[test]
    fn unique_into_inner() {
        let unique = ArcBox::new(10u64);
        assert_eq!(ArcBox::into_inner(unique), 10);
    }

    #[test]
    fn try_from_arc() {
        let x = Arc::new(10_000);
        let y = x.clone();

        assert!(ArcBox::try_from(x).is_err());
        assert_eq!(ArcBox::into_inner(ArcBox::try_from(y).unwrap()), 10_000,);
    }
}
