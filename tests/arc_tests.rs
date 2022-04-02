use elysees::*;
use lazy_static::lazy_static;
use std::borrow::BorrowMut;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Mutex;

#[derive(Debug, Eq, PartialEq, Hash)]
struct SyncPtr(*const ());

unsafe impl Send for SyncPtr {}
unsafe impl Sync for SyncPtr {}

lazy_static! {
    /// Set of roots for MIRI to treat as always reachable, to avoid memory leak errors
    static ref ROOTS: Mutex<HashSet<SyncPtr>> = Mutex::new(HashSet::new());
}

#[test]
fn basic_arc_usage() {
    #![allow(clippy::many_single_char_names)]
    let mut x = Arc::new(7);
    assert!(Arc::is_unique(&x));
    assert_eq!(Arc::get_mut(&mut x), Some(&mut 7));
    assert_eq!(*x, 7);
    let y = x.clone();
    assert!(!Arc::is_unique(&x));
    assert_eq!(Arc::get_mut(&mut x), None);
    assert!(!Arc::is_unique(&y));
    assert_eq!(Arc::load_count(&x, Relaxed), 2);
    assert_eq!(Arc::load_count(&y, Relaxed), 2);
    let x = Arc::try_unique(x).expect_err("x is not unique!");
    assert!(!Arc::is_unique(&x));
    assert!(!Arc::is_unique(&y));
    assert_eq!(x, y);
    assert_eq!(*x, 7);
    std::mem::drop(x);
    assert!(Arc::is_unique(&y));
    let mut y = Arc::try_unique(y).expect("y is unique");
    *y += 73;
    assert_eq!(*y, 80);
    let y = y.shareable();
    assert!(Arc::is_unique(&y));
    assert_eq!(*y, 80);

    let yb = Arc::borrow_arc(&y);
    assert_eq!(*yb, 80);
    assert_eq!(ArcBorrow::load_count(yb, Relaxed), 1);
    let yb2 = yb;
    assert_eq!(ArcBorrow::load_count(yb, Relaxed), 1);
    assert_eq!(ArcBorrow::load_count(yb2, Relaxed), 1);
    let ybr = ArcBorrow::as_arc(&yb2);
    assert_eq!(Arc::load_count(ybr, Relaxed), 1);
    assert!(Arc::is_unique(&ybr));

    let z = y.clone();
    assert_eq!(*z, 80);
    let yl = Arc::leak(y);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 2);
    assert_eq!(Arc::load_count(ArcBorrow::as_arc(&yl), Relaxed), 2);
    assert_eq!(Arc::load_count(&z, Relaxed), 2);
    let t = ArcBorrow::as_arc(&yl).clone();
    assert_eq!(Arc::load_count(&t, Relaxed), 3);
    let w = ArcBorrow::clone_arc(yl);
    assert_eq!(Arc::load_count(&t, Relaxed), 4);
    assert_eq!(Arc::load_count(&z, Relaxed), 4);
    assert_eq!(Arc::load_count(&w, Relaxed), 4);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 4);

    std::mem::drop(w);
    assert_eq!(Arc::load_count(&t, Relaxed), 3);
    assert_eq!(Arc::load_count(&z, Relaxed), 3);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 3);

    std::mem::drop(t);
    assert_eq!(Arc::load_count(&z, Relaxed), 2);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 2);

    std::mem::drop(z);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 1);

    let mut make_unique = Arc::unique(ArcBorrow::clone_arc(yl));
    *make_unique += 23;
    assert_eq!(*make_unique, 103);
    *make_unique.as_mut() += 5;
    assert_eq!(*make_unique, 108);
    let borrowed_unique: &mut usize = make_unique.borrow_mut();
    *borrowed_unique += 5;
    assert_eq!(*borrowed_unique, 113);
    assert_eq!(*make_unique, 113);
    let mut make_unique = make_unique.shareable();
    assert_eq!(*make_unique, 113);
    assert!(Arc::is_unique(&make_unique));
    assert_eq!(*yl, 80);
    let make_mut = Arc::make_mut(&mut make_unique);
    assert_eq!(*make_mut, 113);
    *make_mut += 100;

    assert_eq!(*make_mut, 213);
    assert_eq!(*make_unique, 213);
    assert!(&make_unique != ArcBorrow::as_arc(&yl));
    assert!(&make_unique > ArcBorrow::as_arc(&yl));
    assert!(&make_unique >= ArcBorrow::as_arc(&yl));
    assert!(ArcBorrow::as_arc(&yl) < &make_unique);
    assert!(ArcBorrow::as_arc(&yl) <= &make_unique);
    assert_eq!(ArcBorrow::as_arc(&yl).cmp(&make_unique), Ordering::Less);
    assert_eq!(
        ArcBorrow::as_arc(&yl).partial_cmp(&make_unique),
        Some(Ordering::Less)
    );

    let remake_unique = Arc::try_unique(make_unique).expect("Unique!");
    assert_eq!(*remake_unique, 213);
    let mut box_unique = remake_unique.clone();
    *box_unique += 100;
    assert_eq!(*box_unique, 313);
    assert_eq!(*remake_unique, 213);

    let box_unique = box_unique.shareable();
    assert!(Arc::is_unique(&box_unique));
    let not_unique = box_unique.clone();
    let not_unique = Arc::try_unique(not_unique).expect_err("Not unique!");
    assert!(!Arc::is_unique(&box_unique));
    assert_eq!(Arc::as_ptr(&box_unique), Arc::as_ptr(&not_unique));

    // Avoid memory leak error for yl
    ROOTS
        .lock()
        .unwrap()
        .insert(SyncPtr(ArcBorrow::into_raw(yl) as *const ()));
}

#[test]
fn basic_arc_ref_usage() {
    #![allow(clippy::many_single_char_names)]
    let mut x = ArcRef::new(7);
    assert!(ArcRef::is_unique(&x));
    assert_eq!(ArcRef::get_mut(&mut x), Some(&mut 7));
    assert_eq!(*x, 7);
    let y = x.clone();
    assert!(!ArcRef::is_unique(&x));
    assert_eq!(ArcRef::get_mut(&mut x), None);
    assert!(!ArcRef::is_unique(&y));
    assert_eq!(ArcRef::load_count(&x, Relaxed), 2);
    assert_eq!(ArcRef::load_count(&y, Relaxed), 2);
    let x = ArcRef::try_unique(x).expect_err("x is not unique!");
    assert!(!ArcRef::is_unique(&x));
    assert!(!ArcRef::is_unique(&y));
    assert_eq!(x, y);
    assert_eq!(*x, 7);
    std::mem::drop(x);
    assert!(ArcRef::is_unique(&y));
    let mut y = ArcRef::try_unique(y).expect("y is unique");
    *y += 73;
    assert_eq!(*y, 80);
    let y = y.shareable_ref();
    assert!(ArcRef::is_unique(&y));
    assert_eq!(*y, 80);

    let yb = ArcRef::borrow_arc(&y);
    assert_eq!(*yb, 80);
    assert_eq!(ArcBorrow::load_count(yb, Relaxed), 1);
    let yb2 = yb;
    assert_eq!(ArcBorrow::load_count(yb, Relaxed), 1);
    assert_eq!(ArcBorrow::load_count(yb2, Relaxed), 1);
    let ybr = ArcBorrow::as_arc_ref(&yb2);
    assert_eq!(ArcRef::load_count(ybr, Relaxed), 1);
    assert!(!ArcRef::is_owned(&ybr));
    assert!(!ArcRef::is_unique(&ybr));

    let z = y.clone();
    assert!(ArcRef::is_owned(&z));
    assert_eq!(*z, 80);
    let yl = ArcRef::leak(y);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 2);
    assert_eq!(ArcRef::load_count(ArcBorrow::as_arc_ref(&yl), Relaxed), 2);
    assert_eq!(ArcRef::load_count(&z, Relaxed), 2);
    let t = ArcBorrow::as_arc_ref(&yl).clone();
    assert!(!ArcRef::is_owned(&t));
    assert_eq!(ArcRef::load_count(&t, Relaxed), 2);
    let w = ArcRef::from_arc(ArcBorrow::clone_arc(yl));
    assert!(ArcRef::is_owned(&w));
    assert_eq!(ArcRef::load_count(&t, Relaxed), 3);
    assert_eq!(ArcRef::load_count(&z, Relaxed), 3);
    assert_eq!(ArcRef::load_count(&w, Relaxed), 3);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 3);

    std::mem::drop(w);
    assert_eq!(ArcRef::load_count(&t, Relaxed), 2);
    assert_eq!(ArcRef::load_count(&z, Relaxed), 2);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 2);

    std::mem::drop(t);
    assert_eq!(ArcRef::load_count(&z, Relaxed), 2);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 2);

    std::mem::drop(z);
    assert_eq!(ArcBorrow::load_count(yl, Relaxed), 1);

    let mut make_unique = Arc::unique(ArcBorrow::clone_arc(yl));
    *make_unique += 23;
    assert_eq!(*make_unique, 103);
    *make_unique.as_mut() += 5;
    assert_eq!(*make_unique, 108);
    let borrowed_unique: &mut usize = make_unique.borrow_mut();
    *borrowed_unique += 5;
    assert_eq!(*borrowed_unique, 113);
    assert_eq!(*make_unique, 113);
    let mut make_unique = make_unique.shareable_ref();
    assert_eq!(*make_unique, 113);
    assert!(ArcRef::is_unique(&make_unique));
    assert_eq!(*yl, 80);
    let make_mut = ArcRef::make_mut(&mut make_unique);
    assert_eq!(*make_mut, 113);
    *make_mut += 100;

    assert_eq!(*make_mut, 213);
    assert_eq!(*make_unique, 213);
    assert!(&make_unique != ArcBorrow::as_arc_ref(&yl));
    assert!(&make_unique > ArcBorrow::as_arc_ref(&yl));
    assert!(&make_unique >= ArcBorrow::as_arc_ref(&yl));
    assert!(ArcBorrow::as_arc_ref(&yl) < &make_unique);
    assert!(ArcBorrow::as_arc_ref(&yl) <= &make_unique);
    assert_eq!(ArcBorrow::as_arc_ref(&yl).cmp(&make_unique), Ordering::Less);
    assert_eq!(
        ArcBorrow::as_arc_ref(&yl).partial_cmp(&make_unique),
        Some(Ordering::Less)
    );

    let remake_unique = ArcRef::try_unique(make_unique).expect("Unique!");
    assert_eq!(*remake_unique, 213);
    let mut box_unique = remake_unique.clone();
    *box_unique += 100;
    assert_eq!(*box_unique, 313);
    assert_eq!(*remake_unique, 213);

    let box_unique = box_unique.shareable_ref();
    assert!(ArcRef::is_unique(&box_unique));
    let not_unique = box_unique.clone();
    let not_unique = ArcRef::try_unique(not_unique).expect_err("Not unique!");
    assert!(!ArcRef::is_unique(&box_unique));
    assert_eq!(ArcRef::as_ptr(&box_unique), ArcRef::as_ptr(&not_unique));

    // Avoid memory leak error for yl
    ROOTS
        .lock()
        .unwrap()
        .insert(SyncPtr(ArcBorrow::heap_ptr(yl) as *const ()));
}

/*
#[test]
fn static_arc() {
    static mut ARC_INNER: NonZeroArcInner<u64> = NonZeroArcInner::new(5);
    let arc_inner = unsafe { &mut ARC_INNER };
    let static_borrow = ArcBorrow::new_static(arc_inner);
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 1);
    let static_arc = static_borrow.clone_arc();
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 2);
    assert_eq!(
        Arc::as_ptr(static_borrow.as_arc()),
        Arc::as_ptr(&static_arc)
    );
    drop(static_arc);
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 1);
}

#[test]
fn box_static_arc() {
    let arc_inner = Box::leak(Box::new(NonZeroArcInner::new(5u64)));
    ROOTS
        .lock()
        .unwrap()
        .insert(SyncPtr(arc_inner as *const _ as *const ()));
    let static_borrow = ArcBorrow::new_static(arc_inner);
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 1);
    let static_arc = static_borrow.clone_arc();
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 2);
    assert_eq!(
        Arc::as_ptr(static_borrow.as_arc()),
        Arc::as_ptr(&static_arc)
    );
    drop(static_arc);
    assert_eq!(ArcBorrow::load_count(static_borrow, Relaxed), 1);
}
*/

#[test]
fn from_into_raw() {
    let x = Arc::new(57634);
    let y = x.clone();
    let p = Arc::into_raw(y);
    assert_eq!(p, Arc::as_ptr(&x));
    let y = unsafe { Arc::from_raw(p) };
    let z = ArcRef::from_arc(x.clone());
    let p_ = ArcRef::into_raw(z);
    assert_eq!(p, p_);
    let z = unsafe { Arc::from_raw(p_) };
    assert_eq!(Arc::as_ptr(&x), Arc::as_ptr(&y));
    assert_eq!(Arc::as_ptr(&x), Arc::as_ptr(&z));
}

#[test]
fn arc_formatting() {
    for (arc, name) in &[(Arc::new(56), "56"), (Arc::new(88), "88")] {
        assert_eq!(format!("{}", arc), *name);
        assert_eq!(format!("{:?}", arc), *name);
        assert_eq!(format!("{}", Arc::borrow_arc(arc)), *name);
        assert_eq!(format!("{:?}", Arc::borrow_arc(arc)), *name);
        let _ = format!("{:p}", arc);
    }
}

#[test]
fn arc_default() {
    let arc: Arc<usize> = Arc::default();
    assert_eq!(*arc, 0);
    assert!(Arc::is_unique(&arc));
    let unique_arc: ArcBox<usize> = ArcBox::default();
    assert_eq!(*unique_arc, 0);
}

#[test]
fn arc_hash() {
    let mut map = HashSet::new();
    assert!(map.insert(Arc::new(7)));
    assert!(map.insert(Arc::new(8)));
    assert!(map.insert(Arc::new(9)));
    assert!(!map.insert(Arc::new(7)));

    let mut borrow_map = HashSet::new();
    for arc in map.iter() {
        assert!(borrow_map.insert(Arc::borrow_arc(arc)));
    }
    for arc in map.iter() {
        assert!(!borrow_map.insert(Arc::borrow_arc(arc)));
    }
}

#[test]
fn arc_borrow_cmp() {
    let ints = [645, 6432, 346, 4534];
    for i in ints {
        let x = Arc::new(i);
        for j in ints {
            let y = Arc::new(j);

            let xb = Arc::borrow_arc(&x);
            let yb = Arc::borrow_arc(&y);
            assert_eq!(xb.cmp(&yb), i.cmp(&j));
            assert_eq!(xb.partial_cmp(&yb), i.partial_cmp(&j));
            assert_eq!(xb == yb, i == j);
            assert_eq!(xb != yb, i != j);
            assert_eq!(xb < yb, i < j);
            assert_eq!(xb <= yb, i <= j);
            assert_eq!(xb > yb, i > j);
            assert_eq!(xb >= yb, i >= j);
        }
    }
}

#[test]
fn arc_ref_formatting() {
    for (arc, name) in &[(ArcRef::new(56), "56"), (ArcRef::new(88), "88")] {
        assert_eq!(format!("{}", arc), *name);
        assert_eq!(format!("{:?}", arc), *name);
        let _ = format!("{:p}", arc);
    }
}

#[test]
fn arc_ref_default() {
    let arc: ArcRef<usize> = ArcRef::default();
    assert_eq!(*arc, 0);
    assert!(ArcRef::is_unique(&arc));
}

#[test]
fn arc_ref_hash() {
    let mut map = HashSet::new();
    assert!(map.insert(ArcRef::new(7)));
    assert!(map.insert(ArcRef::new(8)));
    assert!(map.insert(ArcRef::new(9)));
    assert!(!map.insert(ArcRef::new(7)));
}
