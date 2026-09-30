use ir::Inst;
use irparse::parse_ll;

// Two constructors at different priorities run in priority order at the
// top of `main`, ahead of the body.
const TWO_CTORS: &str = r#"
@llvm.global_ctors = appending global [2 x { i32, ptr, ptr }] [{ i32, ptr, ptr } { i32 65535, ptr @_GLOBAL__sub_I_b_cpp, ptr null }, { i32, ptr, ptr } { i32 100, ptr @_GLOBAL__sub_I_a_cpp, ptr null }]
define internal void @_GLOBAL__sub_I_a_cpp() {
  ret void
}
define internal void @_GLOBAL__sub_I_b_cpp() {
  ret void
}
define void @main() {
  ret void
}
"#;

#[test]
fn ctors_run_in_priority_order_ahead_of_main_body() {
    let m = parse_ll(TWO_CTORS);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    let calls: Vec<&str> = main.blocks[0]
        .insts
        .iter()
        .filter_map(|i| match i {
            Inst::Call(c) => Some(c.func.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["_GLOBAL__sub_I_a_cpp", "_GLOBAL__sub_I_b_cpp"]);
}

// A C++ TU's entry is still `_Z4mainv` at irparse time (wholeprog renames
// it later, #457): constructors target it, mirroring that rule.
const CTOR_CPP_ENTRY: &str = r#"
@llvm.global_ctors = appending global [1 x { i32, ptr, ptr }] [{ i32, ptr, ptr } { i32 65535, ptr @_GLOBAL__sub_I_f_cpp, ptr null }]
define internal void @_GLOBAL__sub_I_f_cpp() {
  ret void
}
define i16 @_Z4mainv() {
  ret i16 0
}
"#;

#[test]
fn ctors_target_mangled_entry_before_rename() {
    let m = parse_ll(CTOR_CPP_ENTRY);
    let main = m
        .funcs
        .iter()
        .find(|f| f.name == "_Z4mainv")
        .expect("_Z4mainv");
    match &main.blocks[0].insts[0] {
        Inst::Call(c) => assert_eq!(c.func, "_GLOBAL__sub_I_f_cpp"),
        other => panic!("expected ctor call first, got {other:?}"),
    }
}

// `__cxa_atexit` calls are erased (reset reruns constructors; prior
// dtors must not run), while the guarded init they annotate survives.
const ATEXIT: &str = r#"
define void @main() {
  %1 = call i16 @__cxa_atexit(ptr @d, ptr @g, ptr @__dso_handle)
  ret void
}
"#;

#[test]
fn atexit_calls_are_erased() {
    let m = parse_ll(ATEXIT);
    let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
    assert!(
        main.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Call(c) => Some(c.func.as_str()),
                _ => None,
            })
            .all(|f| f != "__cxa_atexit"),
        "no atexit call may survive"
    );
}

// Guard acquire becomes load/xor/zext into the call's dst; release
// becomes a done-marking store. The frontend's icmp/br is untouched.
const GUARDS: &str = r#"
@_ZGVZ9use_localvE1s = internal global i64 0, align 8
define i8 @use_local() {
  %1 = call i16 @__cxa_guard_acquire(ptr nonnull @_ZGVZ9use_localvE1s)
  %2 = icmp eq i16 %1, 0
  br i1 %2, label %skip, label %init
init:
  call void @__cxa_guard_release(ptr nonnull @_ZGVZ9use_localvE1s)
  br label %skip
skip:
  ret i8 0
}
"#;

#[test]
fn guard_calls_lower_to_init_once_accesses() {
    let m = parse_ll(GUARDS);
    let f = m
        .funcs
        .iter()
        .find(|f| f.name == "use_local")
        .expect("use_local");
    let insts: Vec<&Inst> = f.blocks.iter().flat_map(|b| &b.insts).collect();
    assert!(
        insts
            .iter()
            .all(|i| !matches!(i, Inst::Call(c) if c.func.contains("guard"))),
        "no guard call may survive"
    );
    let mut saw_load = false;
    let mut saw_xor = false;
    let mut saw_zext = false;
    let mut saw_store = false;
    for i in &insts {
        match i {
            Inst::Load(l) if l.ptr == "@_ZGVZ9use_localvE1s" => {
                assert_eq!(l.ty, ir::Ty::I8);
                saw_load = true;
            }
            Inst::Bin(b) if matches!(b.op, ir::BinOp::Xor) => {
                assert_eq!(b.ty, ir::Ty::I8);
                saw_xor = true;
            }
            Inst::Zext(z) if z.dst == "1" => {
                assert_eq!((z.from, z.to), (ir::Ty::I8, ir::Ty::I16));
                saw_zext = true;
            }
            Inst::Store(s) if s.ptr == "@_ZGVZ9use_localvE1s" => {
                assert_eq!(s.val, ir::Val::Const(1));
                saw_store = true;
            }
            _ => {}
        }
    }
    assert!(saw_load, "acquire reads the guard word");
    assert!(saw_xor, "acquire inverts into the result");
    assert!(saw_zext, "result widens into the call dst");
    assert!(saw_store, "release marks done");
}
