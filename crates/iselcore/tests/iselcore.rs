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
#[test]
fn folds_a_pointer_select_over_a_const_base() {
    // `%s = select i1 %c, ptr @addrs+4, ptr @addrs` (the ccp_sel shape):
    // the cond becomes a scale-4 term, the low offset 0 is the base k.
    let m = parse(
        "const addrs i8\n\
         fn main() ()\n\
           block entry:\n\
             %g = gep @addrs +4\n\
             %s = select i1 %c, ptr %g, ptr @addrs\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Global(n) if n == "addrs"));
    assert_eq!(*k, 0);
    assert_eq!(terms, &[(4u8, "c".to_string())]);
}

#[test]
fn folds_a_pointer_select_with_arm_order_swapped() {
    // `select i1 %c, ptr @addrs, ptr @addrs+4` is `c ? 0 : 4` (true->low):
    // iselcore encodes as `base+kb + (ka-kb)*c` with wrapping `u8`, so
    // `kb=4` and `d = 0-4 = 252` (i.e. `4 + 252*c` wraps to `0` when `c=1`).
    // The common `true->hi` shape (`select c, +4, 0`) stays `0 + 4*c`; the
    // swapped shape is correct but large-scale, and `legalize` normalizes it
    // to the small scale in the pipeline.
    let m = parse(
        "global addrs i8\n\
         fn main() ()\n\
           block entry:\n\
             %g = gep @addrs +4\n\
             %s = select i1 %c, ptr @addrs, ptr %g\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Global(n) if n == "addrs"));
    assert_eq!(*k, 4);
    assert_eq!(terms, &[(252u8, "c".to_string())]);
}

#[test]
fn folds_a_noop_pointer_select() {
    // Both arms are the same pointer: the select is a no-op, no term.
    let m = parse(
        "global addrs i8\n\
         fn main() ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @addrs, ptr @addrs\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Global(n) if n == "addrs"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
}

#[test]
fn leaves_a_value_select_unresolved() {
    // A select over runtime regs is a value select (2-byte pointer copy),
    // never a pointer fold: it must not appear in the resolved map.
    let m = parse(
        "fn main() ()\n\
           block entry:\n\
             %s = select i1 %c, i16 %x, i16 %y\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    assert!(
        !r.contains_key("main::s"),
        "a value select must not be resolved as a pointer"
    );
}

#[test]
#[should_panic(expected = "cyclic or unresolvable pointer chain")]
fn panics_on_a_pointer_select_with_distinct_bases() {
    // Arms over different globals cannot fold to one base: loud panic,
    // never a silent read from the wrong table.
    let m = parse(
        "global a i8\n\
         global b i8\n\
         fn main() ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             ret void\n",
    );
    let _ = resolve_pointers(&m);
}
