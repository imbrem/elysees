use arc_swap::RefCnt;

use crate::Arc;

unsafe impl<T> RefCnt for Arc<T> {
    type Base = T;

    #[inline]
    fn into_ptr(me: Self) -> *mut Self::Base {
        Arc::into_raw(me) as *mut _
    }

    #[inline]
    fn as_ptr(me: &Self) -> *mut Self::Base {
        Arc::as_ptr(me) as *mut _
    }

    #[inline]
    unsafe fn from_ptr(ptr: *const Self::Base) -> Self {
        Arc::from_raw(ptr)
    }
}
