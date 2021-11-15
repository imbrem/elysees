# Elysees

Fork of `triomphe`, which is a fork of `Arc`. This has the following advantages over std::sync::Arc:
* `elysees::Arc` doesn't support weak references: we save space by excluding the weak reference count, and we don't do extra read-modify-update operations to handle the possibility of weak references.
* `elysees::UniqueArc` allows one to construct a temporarily-mutable `Arc` which can be converted to a regular `elysees::Arc` later
* `elysees::OffsetArc` can be used transparently from C++ code and is compatible with (and can be converted to/from) `elysees::Arc`
* `elysees::ArcBorrow` is functionally similar to `&elysees::Arc<T>`, however in memory it's simply a (non-owned) pointer to the inner `Arc`. This helps avoid pointer-chasing.
* `elysees::OffsetArcBorrow` is functionally similar to `&elysees::Arc<T>`, however in memory it's simply `&T`. This makes it more flexible for FFI; the source of the borrow need not be an `Arc` pinned on the stack (and can instead be a pointer from C++, or an `OffsetArc`). Additionally, this helps avoid pointer-chasing.
* `elysees::Arc` has can be constructed for dynamically-sized types via `from_header_and_iter`
* `elysees::ArcRef` is a union of an `Arc` and an `ArcBorrow`
