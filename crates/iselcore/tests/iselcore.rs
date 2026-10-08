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
    assert_eq!(terms, &[(1u16, "i".to_string())]);
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
    assert_eq!(terms, &[(4u16, "c".to_string())]);
}

#[test]
fn folds_a_pointer_select_with_arm_order_swapped() {
    // `select i1 %c, ptr @addrs, ptr @addrs+4` (the real HAL emits the
    // offset on the TRUE arm): folds to the same (base, 0, [(4, c)]).
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
    assert_eq!(*k, 0);
    assert_eq!(terms, &[(4u16, "c".to_string())]);
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
fn seeds_a_pointer_select_with_distinct_global_bases() {
    // Arms over different globals cannot fold to one base, but both are
    // runtime address VALUES: the dst is seeded as an indirect slot whose
    // bytes isel materializes as a 2-byte value select (epic-cc#147).
    let m = parse(
        "global a i8\n\
         global b i8\n\
         fn main() ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "s"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_pointer_select_with_a_global_and_a_runtime_slot() {
    // A global arm and a runtime-address reg arm (an IntToPtr dst) do not
    // fold; both are materializable address values, so the dst is seeded
    // as an indirect slot.
    let m = parse(
        "global a i8\n\
         fn main() ()\n\
           block entry:\n\
             %p = inttoptr i16 12 to ptr\n\
             %s = select i1 %c, ptr @a, ptr %p\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "s"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_pointer_select_with_a_nonzero_offset_gep_arm() {
    // epic-cc#781: one arm is a one-hop GEP over a global (`@str+1`),
    // the other a bare global. The arms share no base so they cannot
    // fold, but the offset arm is still a link-time literal (base plus
    // k), so the dst seeds as an indirect slot like the zero-offset
    // cross-base shape.
    let m = parse(
        "global buf i8\n\
         global str i8\n\
         fn main() ()\n\
           block entry:\n\
             %g = gep @str +1\n\
             %s = select i1 %c, ptr %g, ptr @buf\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::s").expect("pointer select must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "s"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_ptr_phi_over_a_ptr_param_and_self_gep() {
    // The sd-card crc walk: `%7 = phi ptr [...]` whose incomings are a
    // ptr param and a GEP over the phi's own dst (the loop increment).
    // Both are runtime address values once the phi seeds as an indirect
    // slot, and the self-GEP then resolves against that seed
    // (epic-cc#143).
    let m = parse(
        "fn add(void) (p=ptr)\n\
           block entry:\n\
             br block5\n\
         block block5:\n\
             %7 = phi ptr %18 block5 %p entry\n\
             %18 = gep %7 +1\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("add::7").expect("ptr phi seeds");
    assert!(matches!(base, Base::Slot(n, true) if n == "7"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("add::18").expect("self-gep resolves");
    assert!(matches!(base, Base::Slot(n, true) if n == "7"));
    assert_eq!(*k, 1);
    assert!(terms.is_empty());
}
#[test]
fn seeds_a_load_ptr_result_as_an_indirect_slot() {
    // `%7 = load ptr, ptr @g_t1_handle` (the HAL's handle field) is a
    // runtime pointer VALUE: its bytes live in the dst slot, so a GEP over
    // it must resolve to that slot (epic-cc#183's TIMER1_IRQHandler shape).
    let m = parse(
        "global g_t1_handle i8\n\
         fn main() ()\n\
           block entry:\n\
             %7 = load ptr @g_t1_handle\n\
             %10 = gep %7 +10\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("main::7").expect("load ptr must seed");
    assert!(matches!(base, Base::Slot(n, true) if n == "7"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("main::10").expect("gep over load ptr must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "7"));
    assert_eq!(*k, 10);
    assert!(terms.is_empty());
}

// epic-cc#131: a `va_arg ptr` result is a runtime pointer VALUE (the
// address bytes live in the dst slot), seeded exactly like an IntToPtr or
// a load-ptr result. A GEP over it must resolve to that slot.
#[test]
fn seeds_a_va_arg_ptr_result_as_an_indirect_slot() {
    let m = parse(
        "fn vprintf(i16) (fmt, ...)\n\
           block entry:\n\
             %s = va_arg ptr ap ptr\n\
             %10 = gep %s +10\n\
             ret i16 0\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("vprintf::s").expect("va_arg ptr must seed");
    assert!(matches!(base, Base::Slot(n, true) if n == "s"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r
        .get("vprintf::10")
        .expect("gep over va_arg ptr must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "s"));
    assert_eq!(*k, 10);
    assert!(terms.is_empty());
}

// A value-typed `va_arg` (i16 read) is a plain value, never a pointer: it
// must not appear in the resolved map.
#[test]
fn leaves_a_value_va_arg_unresolved() {
    let m = parse(
        "fn vprintf(i16) (fmt, ...)\n\
           block entry:\n\
             %9 = va_arg ptr ap i16\n\
             ret i16 %9\n",
    );
    let r = resolve_pointers(&m);
    assert!(
        !r.contains_key("vprintf::9"),
        "a value va_arg must not be resolved as a pointer"
    );
}

// A pointer-typed phi whose incoming values are a pointer PARAM (the
// vprintf forwarding shape: `ap` is a plain ptr param holding the va
// list address) and a GEP over the phi's own dst (the loop increment)
// seeds as an indirect slot: the param's slot holds the address bytes the
// phi copy can move, and the self-GEP resolves against the seed.
#[test]
fn seeds_a_ptr_phi_with_param_and_self_gep_incomings() {
    let m = parse(
        "fn vprintf(i16) (fmt, ap=ptr)\n\
           block entry:\n\
             %p = phi ptr %ap entry %q loop\n\
             %q = gep %p +1\n\
             br loop\n\
           block loop:\n\
             br entry\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("vprintf::p").expect("ptr phi must seed");
    assert!(matches!(base, Base::Slot(n, true) if n == "p"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("vprintf::q").expect("self-gep must resolve");
    assert!(matches!(base, Base::Slot(n, true) if n == "p"));
    assert_eq!(*k, 1);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_ptr_phi_with_a_global_and_self_gep_arm() {
    // The console strcmp walk (epic-cc#610): `%10 = phi ptr [%16, %14],
    // [@lit, %5]` whose incomings are a link-time global address and a
    // GEP over the phi's own dst (the loop increment). The global arm
    // materializes as literals on its edge, so the phi seeds as an
    // indirect slot exactly like the param/self-gep shape, and the
    // self-GEP then resolves against that seed.
    let m = parse(
        "global lit i8\n\
         fn scan(void) ()\n\
           block entry:\n\
             br loop\n\
         block loop:\n\
             %10 = phi ptr %16 loop @lit entry\n\
             %16 = gep %10 +1\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("scan::10").expect("ptr phi seeds");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("scan::16").expect("self-gep resolves");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(*k, 1);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_ptr_phi_with_a_const_gep_and_self_gep_arm() {
    // epic-cc#610: `%207 = phi ptr [%211, %210], [%__gepN, %193]`
    // where `%__gepN = gep @g_line +4` is a materialized inlined-GEP
    // arm (a link-time constant address) and `%211 = gep %207 +1` is
    // the loop increment. The constant arm counts as a runtime address
    // value like a bare global, so the phi seeds and the increment
    // resolves against that seed.
    let m = parse(
        "global lit i8\n\
         fn scan(void) ()\n\
           block entry:\n\
             br loop\n\
         block loop:\n\
             %g1 = gep @lit +4\n\
             %10 = phi ptr %16 loop %g1 entry\n\
             %16 = gep %10 +1\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("scan::10").expect("ptr phi seeds");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("scan::16").expect("self-gep resolves");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(*k, 1);
    assert!(terms.is_empty());
    let (base, k, terms) = r.get("scan::g1").expect("const-gep resolves");
    assert!(matches!(base, Base::Global(n) if n == "lit"));
    assert_eq!(*k, 4);
    assert!(terms.is_empty());
}

#[test]
fn seeds_a_ptr_phi_over_a_folded_select_and_self_gep() {
    // epic-cc#610: `%256 = phi ptr [%270, %266], [%250, %244]` where
    // `%250 = select i1 %c, @g+8, @g+7` folds to a shared base plus a
    // cond term, and `%270 = gep %256 +1` is the loop increment. The
    // folded select counts as a runtime address value, so the phi
    // seeds and the increment resolves against that seed.
    let m = parse(
        "global g i8\n\
         fn scan(void) ()\n\
           block entry:\n\
             %a = gep @g +8\n\
             %b = gep @g +7\n\
             br loop\n\
         block loop:\n\
             %s = select i1 %c, ptr %a, ptr %b\n\
             %10 = phi ptr %16 loop %s entry\n\
             %16 = gep %10 +1\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("scan::10").expect("ptr phi seeds");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(
        *k, 0,
        "entry select carries no offset; the +1 lives on the loop arm"
    );
    assert!(terms.is_empty());
}

#[test]
fn folds_a_select_over_const_gep_arms() {
    let m = parse(
        "global g i8\n\
         fn scan(void) ()\n\
           block entry:\n\
             %a = gep @g +8\n\
             %b = gep @g +7\n\
             %s = select i1 %c, ptr %a, ptr %b\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("scan::s").expect("select folds");
    assert!(matches!(base, Base::Global(n) if n == "g"));
    assert_eq!(*k, 7);
    assert_eq!(terms.len(), 1);
    assert_eq!(terms[0].0, 1);
}

#[test]
fn seeds_a_ptr_phi_over_a_folded_select() {
    let m = parse(
        "global g i8\n\
         fn scan(void) ()\n\
           block entry:\n\
             %a = gep @g +8\n\
             %b = gep @g +7\n\
             %s = select i1 %c, ptr %a, ptr %b\n\
             br loop\n\
         block loop:\n\
             %10 = phi ptr %s loop @g entry\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let (base, k, terms) = r.get("scan::10").expect("ptr phi seeds");
    assert!(matches!(base, Base::Slot(n, true) if n == "10"));
    assert_eq!(*k, 0);
    assert!(terms.is_empty());
}

#[test]
fn single_use_load_into_add_folds_direct() {
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        matches!(folds.loads.get("1"), Some(iselcore::LoadFold::Direct(g)) if g == "in"),
        "load folds to its global: {:?}",
        folds.loads
    );
    assert!(
        folds.forwarded.contains_key("2"),
        "bin result forwards to the store"
    );
}

#[test]
fn two_loads_into_add_folds_only_the_adjacent_one() {
    // `%1` stays staged: its gap holds `%2`'s load, and moving its read
    // past another load would reorder two volatile reads. Widening the
    // gap to co-folding loads is a follow-up; each fold alone is sound.
    let m = parse(
        "global a i8\n\
         global b i8\n\
         global c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %2 %1\n\
             store i8 %3 @c\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        !folds.loads.contains_key("1"),
        "gapped load stays staged: {:?}",
        folds.loads
    );
    assert!(
        matches!(folds.loads.get("2"), Some(iselcore::LoadFold::Direct(g)) if g == "b"),
        "adjacent load folds: {:?}",
        folds.loads
    );
    assert!(
        folds.forwarded.contains_key("3"),
        "add result forwards to the store"
    );
}

#[test]
fn load_fanned_to_two_stores_threads_w() {
    let m = parse(
        "global in i8\n\
         global slot i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             store i8 %1 @slot\n\
             store i8 %1 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        matches!(folds.loads.get("1"), Some(iselcore::LoadFold::ThreadW(g, _)) if g == "in"),
        "load threads W: {:?}",
        folds.loads
    );
    assert!(
        folds.forwarded.is_empty(),
        "multi-use result never forwards"
    );
}

#[test]
fn multi_use_load_into_bin_stays_staged() {
    let m = parse(
        "global in i8\n\
         global out i8\n\
         global out2 i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1 1\n\
             %3 = add i8 %1 2\n\
             store i8 %2 @out\n\
             store i8 %3 @out2\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(!folds.loads.contains_key("1"), "two bin uses keep the slot");
}

#[test]
fn const_source_load_never_folds() {
    let m = parse(
        "const k i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @k\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(!folds.loads.contains_key("1"), "flash reads keep the slot");
    assert!(folds.forwarded.contains_key("2"), "the bin still forwards");
}

#[test]
fn or_producer_never_mirrors_forward() {
    // `Or` binops feed the boolean lanes, whose target keeps its slot:
    // the mirror stays out even where `isel-pic18` itself would fold.
    let m = parse(
        "global a i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = or i8 %1 3\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        !folds.forwarded.contains_key("2"),
        "or keeps its slot for the lanes"
    );
}

#[test]
fn load_before_compare_never_folds() {
    // An icmp-adjacent load may become a lane field reading the source
    // directly; folding it too would read a volatile source twice.
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %c = icmp eq i8 %1 0\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        folds.loads.is_empty(),
        "icmp-adjacent load keeps its slot: {:?}",
        folds.loads
    );
}

#[test]
fn load_into_or_stays_staged_for_the_lanes() {
    let m = parse(
        "global f i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @f\n\
             %2 = or i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        !folds.loads.contains_key("1"),
        "or operands keep their slot"
    );
    assert!(
        !folds.forwarded.contains_key("2"),
        "or results never mirror forward"
    );
}

#[test]
fn banked_source_stays_staged() {
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> = m
        .globals
        .iter()
        .filter(|g| g.name != "in")
        .map(|g| g.name.clone())
        .collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        !folds.loads.contains_key("1"),
        "banked source keeps its slot"
    );
    assert!(folds.forwarded.contains_key("2"), "the bin still forwards");
}

#[test]
fn no_gate_folds_nothing() {
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, None);
    assert!(folds.loads.is_empty(), "bounding pass folds nothing");
    assert!(folds.forwarded.is_empty(), "bounding pass forwards nothing");
}

#[test]
fn pic14_drops_direct_load_but_keeps_forwarded_bin() {
    // The add shape on classic PIC14 (epic-cc#875): `%1` reads `@in`
    // directly, while `%2` still stages: `isel` has no store-folded
    // destination for `Bin` results there.
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert_eq!(
        folds.unplaced_pic14(&m.funcs[0]),
        std::collections::HashSet::from(["1".to_string()]),
    );
}

#[test]
fn pic14_drops_load_forwarded_into_store() {
    // `%1 = load @in; store %1 @slot`: the store reads `@in`, so the
    // copy never stages on PIC14 either.
    let m = parse(
        "global in i8\n\
         global slot i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             store i8 %1 @slot\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        folds.forwarded.contains_key("1"),
        "single-use load forwards to the store"
    );
    assert!(
        folds.unplaced_pic14(&m.funcs[0]).contains("1"),
        "forwarded load needs no slot"
    );
}

#[test]
fn pic14_drops_threaded_load() {
    // A load fanned out to stores rides W on PIC14 just like PIC18:
    // the slot drops either way.
    let m = parse(
        "global in i8\n\
         global slot i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             store i8 %1 @slot\n\
             store i8 %1 @out\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let safe: std::collections::HashSet<String> =
        m.globals.iter().map(|g| g.name.clone()).collect();
    let folds = iselcore::find_value_folds(&m.funcs[0], &m, &r, Some(&safe));
    assert!(
        folds.unplaced_pic14(&m.funcs[0]).contains("1"),
        "threaded load needs no slot"
    );
}

// epic-cc#832: a `load ptr` out of an object's vptr field (a global whose
// loaded bytes all carry nonzero-addend refs) holds a flash address, so
// the slot load through it must read program memory. The dispatch shape
// is a phi/select of object globals feeding the vptr load.
#[test]
fn vptr_load_through_object_phi_is_flash() {
    let mut m = parse(
        "global g_b i8\n\
         global g_d i8\n\
         global g_m i8\n\
         const _ZTVb i8\n\
         const _ZTVd i8\n\
         const _ZTVm i8\n\
         fn main() ()\n\
           block entry:\n\
             %6 = select i1 %c, ptr @g_d, ptr @g_m\n\
             %8 = phi ptr %6 entry @g_b entry\n\
             %9 = load ptr %8\n\
             %10 = load ptr %9\n\
             ret void\n",
    );
    for g in m.globals.iter_mut().filter(|g| !g.is_const) {
        g.refs = vec![
            (0usize, format!("_ZTV{}", g.name[2..].to_string()), 4u16),
            (1, format!("_ZTV{}", g.name[2..].to_string()), 4),
        ];
    }
    let prov = iselcore::flash_provenance(&m);
    assert!(
        prov.flash.contains("main::9"),
        "vptr load holds a flash address, got {:?}",
        prov.flash
    );
    assert!(!prov.flash.contains("main::10"));
    assert!(
        prov.mixed.is_empty(),
        "no partial arm, got {:?}",
        prov.mixed
    );
}

// A `load ptr` straight out of one object global is the same fact with
// no address folding in between.
#[test]
fn vptr_load_direct_from_object_global_is_flash() {
    let mut m = parse(
        "global g_b i8\n\
         const _ZTV4Base i8\n\
         fn main() ()\n\
           block entry:\n\
             %1 = load ptr @g_b\n\
             %2 = load ptr %1\n\
             ret void\n",
    );
    m.globals[0].refs = vec![
        (0usize, "_ZTV4Base".to_string(), 4u16),
        (1, "_ZTV4Base".to_string(), 4),
    ];
    let prov = iselcore::flash_provenance(&m);
    assert!(prov.flash.contains("main::1"));
    assert!(!prov.flash.contains("main::2"));
    assert!(prov.mixed.is_empty());
}

// A `load ptr` out of a plain global (no vptr refs) is the C shape: RAM
// both sides, both provenance sets stay empty.
#[test]
fn plain_global_load_ptr_is_ram() {
    let m = parse(
        "global tbl i8\n\
         fn main() ()\n\
           block entry:\n\
             %1 = load ptr @tbl\n\
             %2 = load ptr %1\n\
             ret void\n",
    );
    let prov = iselcore::flash_provenance(&m);
    assert!(prov.flash.is_empty());
    assert!(prov.mixed.is_empty());
}

// A phi joining a flash (vptr) value with a runtime address may carry
// either: the runtime sequence cannot serve it, so it lands in `mixed`
// and every backend panics rather than emitting a wrong read.
#[test]
fn phi_of_flash_and_runtime_is_mixed() {
    let mut m = parse(
        "global g_b i8\n\
         const _ZTV4Base i8\n\
         fn main() ()\n\
           block entry:\n\
             %9 = load ptr @g_b\n\
             %10 = phi ptr %9 entry %p entry\n\
             %11 = load ptr %10\n\
             ret void\n",
    );
    m.globals[0].refs = vec![
        (0usize, "_ZTV4Base".to_string(), 4u16),
        (1, "_ZTV4Base".to_string(), 4),
    ];
    let prov = iselcore::flash_provenance(&m);
    assert!(prov.flash.contains("main::9"));
    assert!(
        prov.mixed.contains("main::10"),
        "partial-flash phi must be mixed, got {:?}",
        prov.mixed
    );
}

// A plain constant-GEP initializer (`&arr[2]` in C) folds to the same
// nonzero-addend ref shape as a vptr, but names a RAM address: loads out
// of it and dereferences through the result stay RAM (epic-cc#832 review).
#[test]
fn offset_pointer_into_ram_global_is_ram() {
    let mut m = parse(
        "global arr i8\n\
         global p i8\n\
         fn main() ()\n\
           block entry:\n\
             %1 = load ptr @p\n\
             %2 = load ptr %1\n\
             ret void\n",
    );
    m.globals[1].refs = vec![(0usize, "arr".to_string(), 2u16), (1, "arr".to_string(), 2)];
    let prov = iselcore::flash_provenance(&m);
    assert!(prov.flash.is_empty(), "got {:?}", prov.flash);
    assert!(prov.mixed.is_empty(), "got {:?}", prov.mixed);
}
