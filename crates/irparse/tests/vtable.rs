use ir::Inst;
use irparse::parse_ll;

// A C++ vtable decodes like any const table: placeholder bytes with a
// ref per address half. `null` slots fold to zeros; `@f` slots record
// refs that feed the address-taken set (epic-cc#460).
const VTABLE: &str = r#"
@_ZTV7Derived = linkonce_odr dso_local unnamed_addr constant { [3 x ptr] } { [3 x ptr] [ptr null, ptr null, ptr @_ZN7Derived4tickEv] }, comdat, align 2
"#;

#[test]
fn decodes_vtable_const_with_fn_refs() {
    let m = parse_ll(VTABLE);
    assert_eq!(m.globals.len(), 1);
    let vt = &m.globals[0];
    assert_eq!(vt.name, "_ZTV7Derived");
    assert!(vt.is_const, "vtable rides the flash table path (epic-cc#832)");
    assert_eq!(vt.size, 6);
    assert_eq!(vt.bytes, vec![0u8; 6]);
    assert_eq!(
        vt.refs,
        vec![
            (4usize, "_ZN7Derived4tickEv".to_string(), 0u16),
            (5, "_ZN7Derived4tickEv".to_string(), 0),
        ]
    );
}

// An object global's vptr initializer is a constant GEP into its vtable:
// the slot address folds to `(vtable, byte offset)`, not the table base.
const OBJECT_WITH_VPTR: &str = r#"
%class.Base = type { ptr, i8 }
@_ZTV4Base = linkonce_odr dso_local unnamed_addr constant { [3 x ptr] } { [3 x ptr] [ptr null, ptr null, ptr @_ZN4Base4tickEv] }, comdat, align 2
@g_b = dso_local global %class.Base <{ ptr getelementptr inbounds inrange(-4, 2) ({ [3 x ptr] }, ptr @_ZTV4Base, i32 0, i32 0, i32 2), i8 0 }>, align 1
"#;

#[test]
fn folds_vptr_gep_to_slot_address() {
    let m = parse_ll(OBJECT_WITH_VPTR);
    let g = m.globals.iter().find(|g| g.name == "g_b").expect("g_b");
    assert_eq!(
        g.refs,
        vec![
            (0usize, "_ZTV4Base".to_string(), 4u16),
            (1, "_ZTV4Base".to_string(), 4),
        ]
    );
}

// The dispatch sequence (vptr load, slot load, indirect tail call)
// parses with the callee as a numeric indirect target.
const DISPATCH: &str = r#"
@g_b = dso_local global i8 0, align 1
define i8 @main(i8 %0) {
  %1 = load ptr, ptr @g_b, align 1
  %2 = load ptr, ptr %1, align 2
  %3 = tail call i8 %2(ptr %0)
  ret i8 %3
}
"#;

#[test]
fn parses_virtual_dispatch_as_indirect_call() {
    let m = parse_ll(DISPATCH);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("indirect call");
    assert!(call.func.chars().all(|c| c.is_ascii_digit()));
    assert!(call.callees.is_empty(), "callees fill in legalize");
}
