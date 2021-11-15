#[cfg(feature="slice-dst")]
mod dst_tests {

    use elysees::*;
    use erasable::Thin;
    use slice_dst::{SliceWithHeader, TryAllocSliceDst};
    use std::borrow::BorrowMut;
    use std::iter::FromIterator;
    use std::ptr::NonNull;

    #[test]
    fn basic_dst_test() {
        let vec = Vec::from_iter(0..100);
        let arc: Arc<_> = SliceWithHeader::new("header", 0..100);
        assert_eq!(
            std::mem::size_of_val(&arc),
            2 * std::mem::size_of::<usize>()
        );
        assert_eq!(arc.header, "header");
        assert_eq!(arc.slice, vec[..]);
        assert!(arc.is_unique());
        let thin: Thin<_> = arc.clone().into();
        assert_eq!(thin.header, "header");
        assert_eq!(thin.slice, vec[..]);
        assert_eq!(std::mem::size_of_val(&thin), std::mem::size_of::<usize>());
        let borrowed = arc.borrow_arc();
        assert_eq!(
            std::mem::size_of_val(&borrowed),
            2 * std::mem::size_of::<usize>()
        );
        let thin_borrowed: Thin<_> = borrowed.into();
        assert_eq!(
            std::mem::size_of_val(&thin_borrowed),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn unique_dst_test() {
        let mut arc: ArcBox<_> = SliceWithHeader::new("unique", 0..5);
        assert_eq!(
            std::mem::size_of_val(&arc),
            2 * std::mem::size_of::<usize>()
        );
        assert_eq!(arc.header, "unique");
        assert_eq!(arc.slice, [0, 1, 2, 3, 4]);
        arc.slice[3] = 77;
        assert_eq!(arc.slice, [0, 1, 2, 77, 4]);
        arc.as_mut().slice[4] = 88;
        assert_eq!(arc.slice, [0, 1, 2, 77, 88]);
        let mut_borrow: &mut SliceWithHeader<_, _> = arc.borrow_mut();
        mut_borrow.slice[0] = 99;
        assert_eq!(arc.slice, [99, 1, 2, 77, 88]);

        let arc = arc.shareable();
        assert_eq!(arc.header, "unique");
        assert_eq!(arc.slice, [99, 1, 2, 77, 88]);
        let arc = Arc::try_unique(arc).expect("Is unique!");
        assert_eq!(arc.header, "unique");
        assert_eq!(arc.slice, [99, 1, 2, 77, 88]);

        let arc = arc.shareable();
        assert_eq!(arc.header, "unique");
        assert_eq!(arc.slice, [99, 1, 2, 77, 88]);
        let _arc2 = arc.clone();
        Arc::try_unique(arc).expect_err("Not unique!");
    }

    #[cfg(all(feature = "slice-dst", feature = "ptr-union"))]
    #[test]
    fn dst_union_test() {
        let a: Arc<_> = SliceWithHeader::new("a", 0..10);
        let b: ArcBox<_> = SliceWithHeader::new("b", 10..20);
        let c: Arc<_> = SliceWithHeader::new("c", 20..30);
        let d_o: Arc<_> = SliceWithHeader::new("d", 30..40);
        let d = d_o.borrow_arc();

        let mut union2 = UnionAlign::left(a.clone());
        assert!(union2.a().is_some());
        assert!(union2.b().is_none());
        union2 = UnionAlign::right(b);
        assert!(union2.a().is_none());
        assert!(union2.b().is_some());

        let b: ArcBox<_> = SliceWithHeader::new("b", 10..20);

        let mut union4 = UnionAlign::a(a);
        assert!(union4.a().is_some());
        union4 = UnionAlign::b(b);
        assert!(union4.b().is_some());
        union4 = UnionAlign::c(c);
        assert!(union4.c().is_some());
        union4 = UnionAlign::d(d);
        assert!(union4.d().is_some());
    }

    #[test]
    fn failed_dst_alloc() {
        fn fallible_alloc(_: NonNull<SliceWithHeader<&str, u8>>) -> Result<(), &str> {
            Err("Bad!")
        }
        unsafe {
            assert_eq!(Arc::try_new_slice_dst(57, fallible_alloc), Err("Bad!"));
            assert_eq!(ArcBox::try_new_slice_dst(57, fallible_alloc), Err("Bad!"));
        }
    }
}
