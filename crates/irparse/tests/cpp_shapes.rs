use ir::Inst;
use irparse::parse_ll;

// C++ class records lower to `%class.X` (and quoted `%"class.ns::Y"` for
// namespaced classes) with the same `{ ... }` layout as `%struct.X`.
// Previously `build_struct_table` collected only `%struct.`/`%union.`,
// so every class global failed with `unknown struct type %class.Reg`.
const CLASS_GLOBALS: &str = r#"
%class.Acc = type { i8 }
%"class.cfg::Scale" = type { i8 }
@g_acc = dso_local global %class.Acc zeroinitializer, align 1
@g_scale = dso_local global %"class.cfg::Scale" zeroinitializer, align 1
define dso_local void @main() {
  %p = getelementptr inbounds %class.Acc, ptr @g_acc, i16 0, i32 0
  store i8 3, ptr %p, align 1
  %q = getelementptr inbounds %"class.cfg::Scale", ptr @g_scale, i16 0, i32 0
  %v = load i8, ptr %q, align 1
  ret void
}
"#;

#[test]
fn parses_class_types_and_quoted_namespaced_classes() {
    let m = parse_ll(CLASS_GLOBALS);
    assert_eq!(m.globals.len(), 2);
    assert_eq!(m.globals[0].name, "g_acc");
    assert_eq!(m.globals[0].size, 1);
    assert_eq!(m.globals[1].name, "g_scale");
    assert_eq!(m.globals[1].size, 1);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    let mut saw_store = false;
    let mut saw_load = false;
    for inst in main.blocks.iter().flat_map(|b| &b.insts) {
        match inst {
            Inst::Store(s) => {
                assert!(!s.volatile, "plain store carries no marker");
                saw_store = true;
            }
            Inst::Load(l) => {
                assert!(!l.volatile, "plain load carries no marker");
                saw_load = true;
            }
            _ => {}
        }
    }
    assert!(saw_store && saw_load, "GEP-fed access pair must parse");
}

// C1/D1 constructor aliases resolve to their C2/D2 targets instead of
// being dropped with the alias line. The pinned clang calls the target
// directly in every probe so far, so this is tolerance, not load-bearing.
const CTOR_ALIAS: &str = r#"
define linkonce_odr void @_ZN1AC2Ev(ptr %0) {
  ret void
}
@_ZN1AC1Ev = unnamed_addr alias void (ptr), ptr @_ZN1AC2Ev
define void @main() {
  call void @_ZN1AC1Ev(ptr null)
  ret void
}
"#;

#[test]
fn resolves_ctor_alias_to_its_target() {
    let m = parse_ll(CTOR_ALIAS);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    let calls: Vec<&str> = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| match i {
            Inst::Call(c) => Some(c.func.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["_ZN1AC2Ev"]);
}

// `this` and reference params carry `dereferenceable(N)` (and
// `initializes(...)`) on the definition; the call-arg path already
// tolerates both, `parse_param` panicked on the first.
const CPP_PARAMS: &str = r#"
define void @takes(ptr noundef nonnull align 1 dereferenceable(3) %0, ptr initializes((0, 8)) %1) {
  ret void
}
"#;

#[test]
fn accepts_cpp_param_attrs() {
    let m = parse_ll(CPP_PARAMS);
    let f = m.funcs.iter().find(|f| f.name == "takes").expect("takes");
    assert_eq!(f.params.len(), 2);
}

const VOLATILE_ACCESSES: &str = r#"
@flag = dso_local global i8 0, align 1
define void @main() {
  store volatile i8 1, ptr @flag, align 1
  %v = load volatile i8, ptr @flag, align 1
  ret void
}
"#;

#[test]
fn threads_volatile_from_load_store() {
    let m = parse_ll(VOLATILE_ACCESSES);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    let mut saw_store = false;
    let mut saw_load = false;
    for inst in main.blocks.iter().flat_map(|b| &b.insts) {
        match inst {
            Inst::Store(s) => {
                assert!(s.volatile, "volatile store must set the flag");
                saw_store = true;
            }
            Inst::Load(l) => {
                assert!(l.volatile, "volatile load must set the flag");
                saw_load = true;
            }
            _ => {}
        }
    }
    assert!(saw_store && saw_load, "both volatile accesses must parse");
}

const HEAP_NEW: &str = r#"
define void @main() {
  %p = call noalias noundef nonnull ptr @_Znwj(i16 noundef 2)
  ret void
}
"#;

#[test]
#[should_panic(expected = "heap new/delete are not in the EC++ subset")]
fn rejects_heap_new_naming_the_rule() {
    parse_ll(HEAP_NEW);
}

const HEAP_DELETE: &str = r#"
define void @main() {
  call void @_Zdlvj(ptr null)
  ret void
}
"#;

#[test]
#[should_panic(expected = "heap new/delete are not in the EC++ subset")]
fn rejects_heap_delete_naming_the_rule() {
    parse_ll(HEAP_DELETE);
}

const MI_THUNK: &str = r#"
define linkonce_odr noundef i16 @_ZThn4_N1C2fbEv(ptr noundef %0) {
  ret i16 0
}
"#;

#[test]
#[should_panic(expected = "multiple inheritance is not in the EC++ subset")]
fn rejects_mi_thunk_define() {
    parse_ll(MI_THUNK);
}

const MI_THUNK_CALL: &str = r#"
define void @main() {
  %v = call i16 @_ZThn4_N1C2fbEv(ptr null)
  ret void
}
"#;

#[test]
#[should_panic(expected = "multiple inheritance is not in the EC++ subset")]
fn rejects_mi_thunk_call() {
    parse_ll(MI_THUNK_CALL);
}
