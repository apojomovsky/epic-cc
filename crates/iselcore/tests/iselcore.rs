use ir::parse;
use iselcore::{resolve_pointers, ssa_key, Base, Slot};

#[test]
fn slot_direct_returns_the_address() {
    assert_eq!(Slot::Direct(0x42).direct(), 0x42);
}

#[test]
fn ssa_key_joins_function_and_value_names() {
    assert_eq!(ssa_key("main", "1"), "main::1");
}

#[test]
fn resolves_a_global_array_gep() {
    let m = parse(
        "global arr i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @arr +0 +1*%i\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::p").expect("gep must resolve");
    assert!(matches!(base, Base::Global(n) if n == "arr"));
    assert_eq!(*k, 0);
    assert_eq!(terms, &[(1u8, "i".to_string())]);
}

#[test]
fn folds_a_two_link_gep_chain() {
    // %p = gep @arr, k=1 (a struct-field-style offset); %q = gep %p, k=2:
    // the fold must add k (1+2=3) and keep %p's base.
    let m = parse(
        "global arr i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @arr +1\n\
             %q = gep %p +2\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::q").expect("chained gep must resolve");
    assert!(matches!(base, Base::Global(n) if n == "arr"));
    assert_eq!(*k, 3);
    assert!(terms.is_empty());
}

#[test]
fn seeds_alloca_and_byval_sret_params_as_slots() {
    let m = parse(
        "fn f(void) (p=byval2, r=sret)\n\
           block entry:\n\
             %buf = alloca 4\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    assert!(
        matches!(r.get("f::buf"), Some((Base::Slot(n, false), 0, t)) if n == "buf" && t.is_empty())
    );
    assert!(
        matches!(r.get("f::p"), Some((Base::Slot(n, false), 0, t)) if n == "p" && t.is_empty())
    );
    assert!(matches!(r.get("f::r"), Some((Base::Slot(n, true), 0, t)) if n == "r" && t.is_empty()));
}
