use alloc::{allocate, map_text, AllocLayout};
use ir::parse;

/// main calls a and b; each of a and b carries two i16 locals, main carries
/// one i8 local. The overlay must give a and b the same base (never co-live),
/// place main's locals just before that base, and keep the total below the
/// sum of the three functions' individual demands (1 + 4 + 4 = 9).
fn overlay_module() -> ir::Module {
    parse(
        "global in i8\n\
         fn main() -> void\n\
           block entry:\n\
             %m0 = load i8 @in\n\
             call void @a()\n\
             call void @b()\n\
             ret void\n\
         fn a() -> void\n\
           block entry:\n\
             %a0 = add i16 1, 2\n\
             %a1 = add i16 3, 4\n\
             ret void\n\
         fn b() -> void\n\
           block entry:\n\
             %b0 = add i16 5, 6\n\
             %b1 = add i16 7, 8\n\
             ret void\n",
    )
}

#[test]
fn globals_get_bank0_addresses() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = allocate(&m, "depth 1\n");
    assert_eq!(out.globals["in"], 0x20);
    assert_eq!(out.globals["out"], 0x21);
}

#[test]
fn i16_global_advances_two_bytes() {
    let m = parse("global a i8\nglobal b i16\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = allocate(&m, "depth 1\n");
    assert_eq!(out.globals["a"], 0x20);
    assert_eq!(out.globals["b"], 0x22);
}

#[test]
fn sibling_frames_share_a_base() {
    let m = overlay_module();
    let out = allocate(&m, "edge main a\nedge main b\ndepth 2\n");
    // (a) a and b overlay: their i16 locals land on the same addresses.
    assert_eq!(out.locals["a::a0"], out.locals["b::b0"]);
    assert_eq!(out.locals["a::a1"], out.locals["b::b1"]);
    // (b) main's local sits just before a's frame: the regions don't overlap.
    assert_eq!(out.locals["main::m0"], 0x24);
    assert_eq!(out.locals["a::a0"], 0x25);
    assert!(out.locals["main::m0"] < out.locals["a::a0"]);
    // (c) overlay wins: total bank-0 demand < sum of the three demands.
    assert_eq!(out.total_bank0, 5);
    assert!(out.total_bank0 < 1 + 4 + 4);
}

#[test]
fn skips_fn_lines_interspersed_with_edges() {
    // The callgraph binary appends one `fn <name>` line per function after
    // `depth`. The alloc parser must skip them so the documented binary-to-
    // binary workflow keeps working.
    let m = overlay_module();
    let out = allocate(
        &m,
        "depth 2\nfn main\nfn a\nedge main a\nfn b\nedge main b\n",
    );
    // Same overlay result as without the fn lines: a and b share a base.
    assert_eq!(out.locals["a::a0"], out.locals["b::b0"]);
    assert_eq!(out.locals["a::a1"], out.locals["b::b1"]);
}

#[test]
fn map_text_emits_global_and_local_lines() {
    let m = overlay_module();
    let out = allocate(&m, "edge main a\nedge main b\ndepth 2\n");
    assert_eq!(
        map_text(&out),
        "global in 0x20\n\
         local a a0 0x25\n\
         local a a1 0x27\n\
         local b b0 0x25\n\
         local b b1 0x27\n\
         local main m0 0x24\n",
    );
}

#[test]
fn params_are_frame_locals_too() {
    let m = parse(
        "fn f(i16 %p0, i8 %p1) -> void\n\
           block entry:\n\
             %q = add i16 %p0, 1\n\
             ret void\n",
    );
    let out = allocate(&m, "depth 1\n");
    // No globals: end_of_globals = 0x20, so bank0_start = 0x23.
    assert_eq!(out.locals["f::p0"], 0x23);
    assert_eq!(out.locals["f::p1"], 0x25);
    assert_eq!(out.locals["f::q"], 0x26);
    assert_eq!(out.total_bank0, 2 + 1 + 2);
}

#[test]
#[should_panic(expected = "bank 0")]
fn frame_exceeding_bank0_panics() {
    // 60 i16 locals = 120 bytes, more than bank 0 holds.
    let mut src = String::from("fn main() -> void\n  block entry:\n");
    for i in 0..60 {
        src.push_str(&format!("    %v{i} = add i16 1, 2\n"));
    }
    src.push_str("    ret void\n");
    let m = parse(&src);
    let _ = allocate(&m, "depth 1\n");
}

#[test]
fn layout_is_debug_printable_and_default() {
    let l: AllocLayout = Default::default();
    assert!(l.globals.is_empty() && l.locals.is_empty() && l.total_bank0 == 0);
    let _ = format!("{l:?}");
}
