use ir::parse;
use iselcore::resolve_pointers;
use std::collections::HashSet;

// epic-cc#1119: slot address spaces. A global written only with a flash
// const address is flash-kind, and a load-ptr out of it inherits it.
#[test]
fn global_slot_storing_flash_const_is_flash() {
    let m = parse(
        "const c i8\n\
         global g i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store ptr @c @g\n\
             %p = load ptr @g\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c");
    assert!(
        s.contains("@g"),
        "flash-only global slot must be flash: {s:?}"
    );
    assert!(s.contains("main::p"), "load out of it must be flash: {s:?}");
}

// A second writer storing a RAM address prunes the slot back to RAM.
#[test]
fn global_slot_with_ram_writer_is_ram() {
    let m = parse(
        "const c i8\n\
         global g i16\n\
         global buf i8\n\
         fn main(void) ()\n\
           block entry:\n\
             store ptr @c @g\n\
             %q = gep @buf +0\n\
             store ptr %q @g\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c");
    assert!(
        !s.contains("@g"),
        "RAM-written slot must not be flash: {s:?}"
    );
}

// A callee param receiving only a flash const is flash-kind.
#[test]
fn param_receiving_only_flash_const_is_flash() {
    let m = parse(
        "const c i8\n\
         fn use(void) (p=ptr)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @use(ptr @c)\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c");
    assert!(
        s.contains("use::p"),
        "flash-only param must be flash: {s:?}"
    );
}

// One RAM call site prunes the param: the slot joins two spaces.
#[test]
fn param_with_ram_callsite_is_ram() {
    let m = parse(
        "const c i8\n\
         global buf i8\n\
         fn use(void) (p=ptr)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @use(ptr @c)\n\
             call void @use(ptr @buf)\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c");
    assert!(
        !s.contains("use::p"),
        "mixed param must not be flash: {s:?}"
    );
}

// Grounding: slots forwarding each other addresses with no const source
// stay RAM, even though no writer is visibly RAM.
#[test]
fn ungrounded_forwarding_cycle_stays_ram() {
    let m = parse(
        "global g i16\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             store ptr %p @g\n\
             ret void\n\
         fn h(void) ()\n\
           block entry:\n\
             %q = load ptr @g\n\
             call void @f(ptr %q)\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |_| false);
    assert!(
        !s.contains("@g"),
        "ungrounded global must not be flash: {s:?}"
    );
    assert!(
        !s.contains("f::p"),
        "ungrounded param must not be flash: {s:?}"
    );
    assert!(
        !s.contains("h::q"),
        "ungrounded load must not be flash: {s:?}"
    );
}

// A phi over two flash-kind params keeps a flash-kind slot.
#[test]
fn phi_over_flash_params_is_flash() {
    let m = parse(
        "const c i8\n\
         const d i8\n\
         global t i8\n\
         fn walk(void) (p=ptr, q=ptr)\n\
           block entry:\n\
             %l = load i8 @t\n\
             br i1 %l x y\n\
           block x:\n\
             br merge\n\
           block y:\n\
             br merge\n\
           block merge:\n\
             %m = phi ptr %p x %q y\n\
             %v = load i8 %m\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @walk(ptr @c, ptr @d)\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c" || n == "d");
    assert!(s.contains("walk::p"), "got {s:?}");
    assert!(s.contains("walk::q"), "got {s:?}");
    assert!(
        s.contains("walk::m"),
        "phi over flash params must be flash: {s:?}"
    );
}

// An indirect-call target's params gain an unknown caller: RAM.
#[test]
fn indirect_call_target_param_is_ram() {
    let m = parse(
        "const c i8\n\
         fn use(void) (p=ptr)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %f = load i16 @fp\n\
             %b = call i8 %f(ptr @c) callees use\n\
             ret void\n\
         global fp i16\n",
    );
    let r = resolve_pointers(&m);
    let prov = iselcore::flash_provenance(&m);
    let s = iselcore::flash_slot_spaces(&m, &r, &prov, |n| n == "c");
    assert!(
        !s.contains("use::p"),
        "indirect target param must be RAM: {s:?}"
    );
}

#[test]
fn fan_in_of_direct_global_is_itself() {
    let m = parse(
        "const c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let fan = iselcore::const_fan_in(&m, &r, "main", &ir::Val::Global("c".to_string()));
    assert_eq!(fan, HashSet::from(["c".to_string()]));
}

#[test]
fn fan_in_follows_gep_select_and_phi() {
    let m = parse(
        "const a i8\n\
         const b i8\n\
         global buf i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %g = gep @a +1\n\
             %s = select i1 %c, ptr %g, ptr @b\n\
             %p = phi ptr %s entry @buf entry\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let fan = iselcore::const_fan_in(&m, &r, "main", &ir::Val::Reg("g".to_string()));
    assert_eq!(fan, HashSet::from(["a".to_string()]));
    let fan = iselcore::const_fan_in(&m, &r, "main", &ir::Val::Reg("s".to_string()));
    assert_eq!(fan, HashSet::from(["a".to_string(), "b".to_string()]));
    // The phi joins a RAM arm: only the const side is carried.
    let fan = iselcore::const_fan_in(&m, &r, "main", &ir::Val::Reg("p".to_string()));
    assert_eq!(fan, HashSet::from(["a".to_string(), "b".to_string()]));
}

#[test]
fn fan_in_stops_at_params_and_ram() {
    let m = parse(
        "global buf i8\n\
         fn use(void) (p=ptr)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i8 @buf\n\
             ret void\n",
    );
    let r = resolve_pointers(&m);
    let fan = iselcore::const_fan_in(&m, &r, "use", &ir::Val::Reg("p".to_string()));
    assert!(fan.is_empty(), "params carry no const: {fan:?}");
    let fan = iselcore::const_fan_in(&m, &r, "main", &ir::Val::Reg("v".to_string()));
    assert!(fan.is_empty(), "data regs carry no const: {fan:?}");
}
