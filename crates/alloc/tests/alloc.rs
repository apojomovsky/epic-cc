use alloc::{allocate, allocate_with_pool, map_text, AllocLayout};
use device::PIC16F1939;
use device::PIC16F877A;
use device::PIC18F4550;
use ir::parse;

/// main calls a and b; each of a and b carries two i16 locals, main carries
/// one i8 local. The overlay must give a and b the same base (never co-live),
/// place main's locals just before that base, and keep the total below the
/// sum of the three functions' individual demands (1 + 4 + 4 = 9).
fn overlay_module() -> ir::Module {
    parse(
        "global in i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %m0 = load i8 @in\n\
             call void @a()\n\
             call void @b()\n\
             ret void\n\
         fn a(void) ()\n\
           block entry:\n\
             %a0 = add i16 1, 2\n\
             %a1 = add i16 3, 4\n\
             ret void\n\
         fn b(void) ()\n\
           block entry:\n\
             %b0 = add i16 5, 6\n\
             %b1 = add i16 7, 8\n\
             ret void\n",
    )
}

#[test]
fn globals_get_bank0_addresses() {
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    ret void\n");
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["in"], 0x20);
    assert_eq!(out.globals["out"], 0x21);
}

#[test]
fn i16_global_advances_two_bytes() {
    // The prefer-lower-footprint tie-break (epic-hal#86): the largest-first
    // bin-pack places the 2-byte i16 first at 0x20 and the 1-byte i8 at
    // 0x22, one byte tighter than the .ll-order sequential (i8 at 0x20,
    // i16 at 0x22-0x23). The i16 still advances by two bytes.
    let m = parse("global a i8\nglobal b i16\nfn main(void) ()\n  block entry:\n    ret void\n");
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["b"], 0x20);
    assert_eq!(out.globals["a"], 0x22);
}

#[test]
fn large_array_after_i16_lands_at_next_even_and_spans_sequentially() {
    // An i16 global at 0x20-0x21 advances the free pointer to 0x22; an 8-byte
    // array placed after it must land at the next even address (0x22), NOT
    // be 8-byte-aligned to 0x28 (which would waste 0x22-0x27). The array's
    // span is sequential, so the following global starts at 0x2A.
    let mut m = parse(
        "global a i16\n\
         global arr i8\n\
         global after i8\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[1].size = 8; // arr: [8 x i8]
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["a"], 0x20);
    assert_eq!(out.globals["arr"], 0x22, "array must reuse the even slot");
    // a spans 0x20-0x21, arr spans 0x22-0x29: the next global is sequential.
    assert_eq!(out.globals["after"], 0x2A);
}

#[test]
fn sibling_frames_share_a_base() {
    let m = overlay_module();
    let out = allocate(&PIC16F877A, &m, "edge main a\nedge main b\ndepth 2\n");
    // (a) a and b overlay: their i16 locals land on the same addresses.
    assert_eq!(out.locals["a::a0"], out.locals["b::b0"]);
    assert_eq!(out.locals["a::a1"], out.locals["b::b1"]);
    // (b) main's local sits just before a's frame: the regions don't overlap.
    // Frames start at end_of_globals (0x21) since scratch/retval live in
    // fixed common RAM (0x70-0x72), not after the globals.
    assert_eq!(out.locals["main::m0"], 0x21);
    assert_eq!(out.locals["a::a0"], 0x22);
    assert!(out.locals["main::m0"] < out.locals["a::a0"]);
    // The dead defs (a0/a1/b0/b1 never read) share one slot each, so the
    // total is 3 (main's m0 + the shared 2-byte slot), not the 5 the
    // pre-liveness allocator needed.
    assert_eq!(out.total_bank0, 3);
    assert!(out.total_bank0 < 1 + 4 + 4);
}

#[test]
fn skips_fn_lines_interspersed_with_edges() {
    // The callgraph binary appends one `fn <name>` line per function after
    // `depth`. The alloc parser must skip them so the documented binary-to-
    // binary workflow keeps working.
    let m = overlay_module();
    let out = allocate(
        &PIC16F877A,
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
    let out = allocate(&PIC16F877A, &m, "edge main a\nedge main b\ndepth 2\n");
    assert_eq!(
        map_text(&out),
        "global in 0x20\n\
         local a a0 0x22\n\
         local a a1 0x22\n\
         local b b0 0x22\n\
         local b b1 0x22\n\
         local main m0 0x21\n",
    );
}

#[test]
fn params_are_frame_locals_too() {
    let m = parse(
        "fn f(void) (p0=byval2, p1=i8)\n\
           block entry:\n\
             %q = add i16 %p0, 1\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // No globals: end_of_globals = 0x20, so bank0_start = 0x20 (scratch/retval
    // live in fixed common RAM, not after the globals). Locals are placed
    // contiguously (M3 overlay math): p0 i16 at 0x20, p1 i8 at 0x22, q i16 at
    // 0x23 — no intra-frame i16 alignment.
    assert_eq!(out.locals["f::p0"], 0x20);
    assert_eq!(out.locals["f::p1"], 0x22);
    assert_eq!(out.locals["f::q"], 0x23);
    assert_eq!(out.total_bank0, 2 + 1 + 2);
}

#[test]
fn globals_span_across_banks() {
    // 90 i8 globals = 90 bytes: bank 0 GPR holds 80 (0x20-0x6F), so the 81st
    // global lands at the start of bank 1 (0xA0).
    let mut gsrc = String::new();
    for i in 0..90 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let src = format!("{gsrc}fn main(void) ()\n  block entry:\n    ret void\n");
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["g0"], 0x20);
    assert_eq!(out.globals["g79"], 0x6F); // last bank-0 GPR byte
    assert!(
        out.globals["g80"] >= 0xA0,
        "81st global crosses into bank 1"
    );
    assert_eq!(out.globals["g80"], 0xA0);
    assert!(out.globals["g89"] >= 0xA0);
}

#[test]
fn frame_spans_across_banks() {
    // One function with 90 i8 locals, all live (stored to a const sink so
    // liveness keeps them co-resident): its frame crosses bank 0 into 0xA0+.
    let mut src = String::from("const sink i8\nfn f(void) ()\n  block entry:\n");
    for i in 0..90 {
        src.push_str(&format!("    %v{i} = add i8 1, 2\n"));
    }
    for i in 0..90 {
        src.push_str(&format!("    store i8 %v{i}, ptr @sink\n"));
    }
    src.push_str("    ret void\n");
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // No globals: the root frame starts at 0x20; v79 at 0x6F, v80 at 0xA0.
    assert_eq!(out.locals["f::v0"], 0x20);
    assert_eq!(out.locals["f::v79"], 0x6F);
    assert!(
        out.locals["f::v80"] >= 0xA0,
        "80th local crosses into bank 1"
    );
    assert_eq!(out.locals["f::v80"], 0xA0);
}

#[test]
fn callee_base_follows_callers_physical_frame_end() {
    // main (1 i8 local) calls a, which carries 90 i8 locals that spill across
    // the bank-0/1 gap (0x21..0x6F then 0xA0..0xAA); a calls b. b's overlay
    // base must be a's PHYSICAL frame end (0xAB, just past a's last local) —
    // not the virtual sum base(a) + locals_size(a) = 0x7B, which lands in the
    // common-RAM gap and would place b at 0xA0, exactly where a's spill
    // locals live while both frames are live during the call.
    let mut src = String::from(
        "const sink i8\nfn main(void) ()\n\
           block entry:\n\
             %m0 = add i8 1, 2\n\
             call void @a()\n\
             ret void\n\
         fn a(void) ()\n\
           block entry:\n",
    );
    for i in 0..90 {
        src.push_str(&format!("    %v{i} = add i8 1, 2\n"));
    }
    for i in 0..90 {
        src.push_str(&format!("    store i8 %v{i}, ptr @sink\n"));
    }
    src.push_str(
        "    call void @b()\n\
           ret void\n\
         fn b(void) ()\n\
           block entry:\n\
             %b0 = add i8 1, 2\n\
             ret void\n",
    );
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "edge main a\nedge a b\n");
    // main's frame: base 0x20, its local at 0x20, physical end 0x21.
    assert_eq!(out.locals["main::m0"], 0x20);
    // a's frame starts right after main's physical end and crosses the bank
    // gap: v0..v78 in bank 0 (0x21..0x6F), v79..v89 in bank 1 (0xA0..0xAA).
    assert_eq!(out.locals["a::v0"], 0x21);
    assert_eq!(out.locals["a::v78"], 0x6F);
    assert_eq!(out.locals["a::v79"], 0xA0);
    assert_eq!(out.locals["a::v89"], 0xAA);
    // b's base is a's physical frame end (0xAB): b's local starts strictly
    // after a's last placed local, not at 0xA0 where a's spill locals live.
    assert_eq!(out.locals["b::b0"], 0xAB);
    assert!(
        out.locals["b::b0"] > out.locals["a::v89"],
        "b's frame overlaps a's spill locals in bank 1"
    );
}

#[test]
fn callee_base_clears_region_tail_hole_left_by_i16_local() {
    // 79 i8 globals fill 0x20..0x6E, so end_of_globals = 0x6F and main's root
    // frame starts at 0x6F — the last byte of bank 0. main's i16 local does
    // not fit in the single remaining bank-0 byte, so place_contiguous moves
    // it *wholesale* to 0xA0 (leaving the 0x6F byte as an unused hole), then
    // the i8 local lands at 0xA2: main's TRUE physical end is 0xA3. A
    // contiguous-blob frame_end(0x6F, 3) would count the hole byte and stop
    // at 0xA2, and a callee b based on that would land exactly on main's
    // live v1 (silent miscompile). The frame end must come from the actually
    // placed locals: b's base is 0xA3, strictly after main::v1.
    let mut gsrc = String::new();
    for i in 0..79 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let src = format!(
        "{gsrc}const sink i8\nfn main(void) ()\n\
           block entry:\n\
             %v0 = add i16 1, 2\n\
             %v1 = add i8 3, 4\n\
             store i16 %v0, ptr @sink\n\
             store i8 %v1, ptr @sink\n\
             call void @b()\n\
             ret void\n\
         fn b(void) ()\n\
           block entry:\n\
             %b0 = add i8 1, 2\n\
             ret void\n"
    );
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "edge main b\n");
    // 79 i8 globals: the last one at 0x6E, so the root frame starts at 0x6F.
    assert_eq!(out.globals["g78"], 0x6E);
    // main's frame: i16 at 0xA0-0xA1 (bank 1, since 0x6F cannot hold it),
    // i8 at 0xA2 — true physical end 0xA3.
    assert_eq!(out.locals["main::v0"], 0xA0);
    assert_eq!(out.locals["main::v1"], 0xA2);
    // b's base is main's true physical end, not the blob-model 0xA2 that
    // overlays main::v1.
    assert_eq!(out.locals["b::b0"], 0xA3);
    assert!(
        out.locals["b::b0"] > out.locals["main::v1"],
        "b's frame overlaps main's live local v1 at 0xA2"
    );
}

#[test]
fn i16_globals_stay_even_aligned_across_banks() {
    // 80 i8 globals fill bank 0 (0x20-0x6F); the next i16 must land on an
    // even address in bank 1 (0xA0, not 0xA1).
    let mut gsrc = String::new();
    for i in 0..80 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let src = format!("{gsrc}global w i16\nfn main(void) ()\n  block entry:\n    ret void\n");
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["g79"], 0x6F);
    assert!(out.globals["w"] >= 0xA0, "i16 spills into bank 1");
    assert_eq!(
        out.globals["w"] % 2,
        0,
        "i16 stays even-aligned within the bank"
    );
}

#[test]
fn i16_frame_stays_even_aligned_across_banks() {
    // A frame of 50 i16 locals (100 bytes) crosses bank 0 into bank 1. From
    // the even root base 0x20 the i16s land on even addresses, and the bank
    // progression (0x6F -> 0xA0) keeps them even-aligned within each bank.
    let mut src = String::from("const sink i16\nfn f(void) ()\n  block entry:\n");
    for i in 0..50 {
        src.push_str(&format!("    %v{i} = add i16 1, 2\n"));
    }
    for i in 0..50 {
        src.push_str(&format!("    store i16 %v{i}, ptr @sink\n"));
    }
    src.push_str("    ret void\n");
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    for i in 0..50 {
        let a = out.locals[&format!("f::v{i}")];
        assert_eq!(a % 2, 0, "i16 local v{i} at {a:#04x} must be even");
    }
    // v40 is the 41st i16 (80 bytes in), the first to spill into bank 1.
    assert_eq!(out.locals["f::v40"], 0xA0);
    assert!(out.locals["f::v40"] >= 0xA0);
}

#[test]
fn const_globals_get_no_address_and_sized_globals_span() {
    // `global ram i8` with size 8 spans 8 addresses (0x20..0x27); `const
    // table i8` (size 4) gets NO RAM address. The map lists the RAM global
    // with its address and the const global without one, so isel can see both.
    let mut m = parse(
        "global ram i8\n\
         const table i8\n\
         global after i8\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    // Array sizes come from irparse (LLVM `[N x T]`); the simple parser
    // sizes by type, so set them explicitly to mirror a real module.
    m.globals[0].size = 8; // ram: [8 x i8]
    m.globals[1].size = 4; // table: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // ram is sized by Global.size (8), not ty.bytes() (1): it spans 8 bytes.
    assert_eq!(out.globals["ram"], 0x20);
    // The next RAM global starts after ram's 8 bytes.
    assert_eq!(out.globals["after"], 0x28);
    // table is const: no RAM address.
    assert!(!out.globals.contains_key("table"));
    let text = map_text(&out);
    assert!(
        text.contains("global ram 0x20\n"),
        "map must address ram:\n{text}"
    );
    assert!(
        text.contains("const table\n"),
        "map must list const table without an address:\n{text}"
    );
}

#[test]
fn sized_array_global_does_not_break_frame_overlay() {
    // An 8-byte array global consumes 8 addresses; the root frame's locals
    // must start after it (0x28), not after a 1-byte type, and a callee
    // overlaid on that frame must respect the sized end_of_globals.
    let mut m = parse(
        "global ram i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %m0 = add i8 1, 2\n\
             call void @a()\n\
             ret void\n\
         fn a(void) ()\n\
           block entry:\n\
             %a0 = add i8 1, 2\n\
             ret void\n",
    );
    m.globals[0].size = 8; // ram: [8 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main a\n");
    // ram spans 0x20..0x27; main's local starts at 0x28.
    assert_eq!(out.globals["ram"], 0x20);
    assert_eq!(out.locals["main::m0"], 0x28);
    // a overlays main's frame at main's physical end (0x29).
    assert_eq!(out.locals["a::a0"], 0x29);
}

#[test]
fn const_select_arms_are_copied_to_ram_when_the_select_does_not_fold() {
    // A pointer select over two distinct const globals is a runtime
    // address VALUE (iselcore seeds it as an indirect slot, epic-cc#147):
    // the selected arm's bytes are read through the slot with RAM
    // semantics, so both const arms must be copied to RAM.
    let mut m = parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             ret void\n",
    );
    m.globals[0].size = 4; // a: [4 x i8]
    m.globals[1].size = 4; // b: [4 x i8]
    let out = allocate(&PIC18F4550, &m, "depth 1\n");
    assert!(
        out.globals.contains_key("a"),
        "const select arm @a must be copied to RAM"
    );
    assert!(
        out.globals.contains_key("b"),
        "const select arm @b must be copied to RAM"
    );
}

#[test]
fn a_single_const_select_arm_is_copied_to_ram() {
    // A select with one const arm and one RAM global arm does not fold
    // (iselcore seeds it as an indirect slot): the const arm's bytes are
    // read through the slot with RAM semantics, so it must be copied to
    // RAM even though the other arm is already RAM.
    let mut m = parse(
        "const a i8\n\
         global b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             ret void\n",
    );
    m.globals[0].size = 4; // a: [4 x i8]
    let out = allocate(&PIC18F4550, &m, "depth 1\n");
    assert!(
        out.globals.contains_key("a"),
        "const select arm @a must be copied to RAM"
    );
    assert!(
        out.globals.contains_key("b"),
        "RAM arm @b keeps its address"
    );
}

#[test]
fn const_direct_ptr_call_arg_is_copied_to_ram() {
    // Menu-demo `.str` (epic-cc#754): a const passed directly as a plain
    // pointer call argument is read through the generic pointer path, so
    // it needs a RAM address; flash tables need TBLPTR, not FSR/INDF.
    let mut m = parse(
        "const c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@c)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8], like menu-demo .str
    let out = allocate(&PIC18F4550, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("c"),
        "const ptr call arg @c must be copied to RAM"
    );
    assert!(
        !out.const_globals.contains("c"),
        "copied const @c must leave the flash set"
    );
}

#[test]
fn const_gep_ptr_call_arg_is_copied_to_ram() {
    // Same need one GEP hop out: the scan walks reg chains to the const
    // base, so a derived pointer passed as a plain call arg copies it too.
    let mut m = parse(
        "const c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %g = gep @c +1\n\
             call void @f(%g)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 4; // c: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("c"),
        "const base @c behind a GEP call arg must be copied to RAM"
    );
    assert!(
        !out.const_globals.contains("c"),
        "copied const @c must leave the flash set"
    );
}

#[test]
fn const_gep_select_arm_is_copied_to_ram() {
    // The select branch walks the same reg chain: a GEP-derived arm keeps
    // the select from folding, so the const base is copied like a call arg.
    // Offset +0: only a zero-offset GEP seeds as a runtime value in
    // iselcore; the nonzero cross-base shape is epic-cc#781, not this pin.
    let mut m = parse(
        "const c i8\n\
         global b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %d = icmp eq i8 1, 1\n\
             %g = gep @c +0
             %s = select i1 %d, ptr %g, ptr @b\n\
             ret void\n",
    );
    m.globals[0].size = 4; // c: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert!(
        out.globals.contains_key("c"),
        "const base @c behind a GEP select arm must be copied to RAM"
    );
    assert!(
        !out.const_globals.contains("c"),
        "copied const @c must leave the flash set"
    );
}

#[test]
fn const_to_ram_set_holds_only_pointer_path_consts() {
    // The surviving set, pinned: a const flowing through a generic pointer
    // path is copied, one that never does stays in flash. A future const
    // joining RAM must update this test, never slip in silently.
    let mut m = parse(
        "const used i8\n\
         const table i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@used)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // used: [2 x i8]
    m.globals[1].size = 4; // table: [4 x i8]
    let out = allocate(&PIC18F4550, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("used"),
        "const ptr call arg @used must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("table"),
        "flash-only const @table must get no RAM address"
    );
    assert!(
        out.const_globals.contains("table"),
        "flash-only const @table must stay in the flash set"
    );
    assert!(
        !out.const_globals.contains("used"),
        "copied const @used must leave the flash set"
    );
}

#[test]
fn const_large_ptr_call_arg_stays_in_flash() {
    // The 255-byte ceiling on every const_to_ram insertion: a large const
    // used as a plain call arg still gets no RAM address. Deleting the
    // guard must fail here, not just on the unreferenced 300-byte table.
    let mut m = parse(
        "const big i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@big)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 300; // big: [300 x i8] const table, referenced
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        !out.globals.contains_key("big"),
        "large const @big must not be copied to RAM"
    );
    assert!(
        out.const_globals.contains("big"),
        "large const @big must stay in the flash set"
    );
}

#[test]
fn staged_const_call_args_share_one_buffer() {
    // Small cores (epic-cc#790): directly named const call args stage
    // through one shared buffer instead of per-copy RAM. The buffer sizes
    // to the largest staged const: after (1B) at 0x20, 5 bytes of stage,
    // then f::p (2B param), so bank 0 holds 8 bytes.
    let mut m = parse(
        "const c i8\n\
         const big i8\n\
         global after i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@c)\n\
             call void @f(@big)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 5; // big: [5 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        !out.globals.contains_key("c"),
        "staged const @c must get no RAM copy"
    );
    assert!(
        !out.globals.contains_key("big"),
        "staged const @big must get no RAM copy"
    );
    assert!(
        out.const_globals.contains("c") && out.const_globals.contains("big"),
        "staged consts stay in the flash set"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "one shared staging buffer must be placed"
    );
    assert_eq!(
        out.bank_used[0], 8,
        "buffer sizes to the largest staged const (1 + 5 + 2 param)"
    );
}

#[test]
fn staged_const_with_derived_use_keeps_its_copy() {
    // A const reached both directly and through a GEP chain keeps its RAM
    // copy: the derived path needs a real address isel can name.
    let mut m = parse(
        "const c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %g = gep @c +1\n\
             call void @f(@c)\n\
             call void @f(%g)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 4; // c: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("c"),
        "derived-reached const @c must be copied to RAM"
    );
    assert!(
        !out.const_globals.contains("c"),
        "copied const @c must leave the flash set"
    );
}

#[test]
fn staged_direct_select_arms_share_one_buffer() {
    // Directly named select arms stage like direct call args: no per-arm
    // RAM copies, one buffer sized to the larger arm.
    let mut m = parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             call void @f(%s)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 3; // a: [3 x i8]
    m.globals[1].size = 4; // b: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        !out.globals.contains_key("a") && !out.globals.contains_key("b"),
        "staged select arms must get no RAM copies"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "one shared staging buffer must be placed"
    );
}

#[test]
fn staged_const_call_arg_on_pic14e() {
    // The staging rule covers both small GPR cores; two consts stage,
    // while a lone one keeps its copy (see the count rule).
    let mut m = parse(
        "const c i8\n\
         const d i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@c)\n\
             call void @f(@d)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 3; // d: [3 x i8]
    let out = allocate(&PIC16F1939, &m, "edge main f\n");
    assert!(
        !out.globals.contains_key("c") && !out.globals.contains_key("d"),
        "staged consts must get no RAM copies"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "one shared staging buffer must be placed"
    );
}

#[test]
fn lone_const_call_arg_keeps_its_copy() {
    // One const's copy is cheaper than a buffer: no staging, no buffer.
    let mut m = parse(
        "const c i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@c)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("c"),
        "lone const @c must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("__const_stage"),
        "no staging buffer without a paying set"
    );
}

#[test]
fn multi_const_call_keeps_copies() {
    // One call naming two staged consts would leave both params reading
    // the last copy: demote both to per-copies, no buffer.
    let mut m = parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@a, @b)\n\
             ret void\n\
         fn f(void) (p=ptr, q=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 4; // a: [4 x i8]
    m.globals[1].size = 4; // b: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("a") && out.globals.contains_key("b"),
        "shared-call consts must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("__const_stage"),
        "no staging buffer without a paying set"
    );
}

#[test]
fn escaping_const_call_arg_keeps_its_copy() {
    // A callee that stores its param keeps the buffer address alive past
    // the next staging: the stored const demotes to a per-copy while the
    // readers still share the buffer.
    let mut m = parse(
        "const c i8\n\
         const a i8\n\
         const b i8\n\
         global sink i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @save(@c)\n\
             call void @read(@a)\n\
             call void @read(@b)\n\
             ret void\n\
         fn save(void) (p=ptr)\n\
           block entry:\n\
             store ptr %p, ptr @sink\n\
             ret void\n\
         fn read(void) (q=ptr)\n\
           block entry:\n\
             %v = load i8 %q\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 3; // a: [3 x i8]
    m.globals[2].size = 4; // b: [4 x i8]
    m.globals[3].size = 2; // sink: one pointer
    let out = allocate(&PIC16F877A, &m, "edge main save\nedge main read\n");
    assert!(
        out.globals.contains_key("c"),
        "stored const @c must keep its RAM copy"
    );
    assert!(
        !out.const_globals.contains("c"),
        "stored const @c must leave the flash set"
    );
    assert!(
        !out.globals.contains_key("a") && !out.globals.contains_key("b"),
        "reader consts must still stage with no RAM copies"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "readers still pay for the shared buffer"
    );
}

#[test]
fn forwarded_const_call_arg_keeps_its_copy() {
    // A callee that forwards its param into another call hands the buffer
    // address onward: the forwarded const demotes to a per-copy while the
    // readers still share the buffer.
    let mut m = parse(
        "const c i8\n\
         const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @fwd(@c)\n\
             call void @read(@a)\n\
             call void @read(@b)\n\
             ret void\n\
         fn fwd(void) (p=ptr)\n\
           block entry:\n\
             call void @inner(%p)\n\
             ret void\n\
         fn inner(void) (r=ptr)\n\
           block entry:\n\
             %v = load i8 %r\n\
             ret void\n\
         fn read(void) (q=ptr)\n\
           block entry:\n\
             %w = load i8 %q\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 3; // a: [3 x i8]
    m.globals[2].size = 4; // b: [4 x i8]
    let out = allocate(
        &PIC16F877A,
        &m,
        "edge main fwd\nedge main read\nedge fwd inner\n",
    );
    assert!(
        out.globals.contains_key("c"),
        "forwarded const @c must keep its RAM copy"
    );
    assert!(
        !out.globals.contains_key("a") && !out.globals.contains_key("b"),
        "reader consts must still stage with no RAM copies"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "readers still pay for the shared buffer"
    );
}

#[test]
fn indirect_const_call_arg_keeps_its_copy() {
    // An indirect call has no visible callee, so the address could survive
    // anywhere: the const demotes to a per-copy while the direct-call
    // readers still share the buffer.
    let mut m = parse(
        "const c i8\n\
         const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %fp = add i16 1, 2\n\
             call void %fp(@c) callees read\n\
             call void @read(@a)\n\
             call void @read(@b)\n\
             ret void\n\
         fn read(void) (q=ptr)\n\
           block entry:\n\
             %v = load i8 %q\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 3; // a: [3 x i8]
    m.globals[2].size = 4; // b: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main read\n");
    assert!(
        out.globals.contains_key("c"),
        "indirect-call const @c must keep its RAM copy"
    );
    assert!(
        !out.globals.contains_key("a") && !out.globals.contains_key("b"),
        "reader consts must still stage with no RAM copies"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "readers still pay for the shared buffer"
    );
}

#[test]
fn multi_use_select_keeps_copies() {
    // A select dst consumed twice cannot stage: the second use would read
    // a re-staged buffer. Demote both arms to per-copies.
    let mut m = parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             call void @f(%s)\n\
             call void @f(%s)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 4; // a: [4 x i8]
    m.globals[1].size = 4; // b: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("a") && out.globals.contains_key("b"),
        "multi-use select arms must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("__const_stage"),
        "no staging buffer without a paying set"
    );
}

#[test]
fn cross_block_select_use_keeps_copies() {
    // A select consumed in another block cannot stage: control flow could
    // re-stage between. Demote both arms to per-copies.
    let mut m = parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %s = select i1 %c, ptr @a, ptr @b\n\
             br label %next\n\
           block next:\n\
             call void @f(%s)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 4; // a: [4 x i8]
    m.globals[1].size = 4; // b: [4 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        out.globals.contains_key("a") && out.globals.contains_key("b"),
        "cross-block select arms must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("__const_stage"),
        "no staging buffer without a paying set"
    );
}

#[test]
fn isr_const_use_keeps_copies() {
    // Uses under an ISR could run between another site's staging and its
    // call: demote them. Main's own pair still stages with its buffer.
    let mut m = parse(
        "const c i8\n\
         const d i8\n\
         const e i8\n\
         fn handler(void) [isr] ()\n\
           block entry:\n\
             call void @f(@c)\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@d)\n\
             call void @f(@e)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 2; // d: [2 x i8]
    m.globals[2].size = 2; // e: [2 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main f\nedge handler f\n");
    assert!(
        out.globals.contains_key("c"),
        "ISR-reached const @c must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("d") && !out.globals.contains_key("e"),
        "main's own pair still stages"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "one shared staging buffer must be placed"
    );
}

#[test]
fn addrtaken_const_use_keeps_copies() {
    // A function passed as a value could run under an ISR dispatch the
    // call graph cannot see: demote its const uses, as with ISR roots.
    let mut m = parse(
        "const c i8\n\
         const d i8\n\
         const e i8\n\
         fn cb(void) ()\n\
           block entry:\n\
             call void @f(@c)\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @t(@cb)\n\
             call void @f(@d)\n\
             call void @f(@e)\n\
             ret void\n\
         fn t(void) (p=ptr)\n\
           block entry:\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].size = 2; // c: [2 x i8]
    m.globals[1].size = 2; // d: [2 x i8]
    m.globals[2].size = 2; // e: [2 x i8]
    let out = allocate(&PIC16F877A, &m, "edge main t\nedge main f\nedge cb f\n");
    assert!(
        out.globals.contains_key("c"),
        "address-taken const @c must be copied to RAM"
    );
    assert!(
        !out.globals.contains_key("d") && !out.globals.contains_key("e"),
        "main's own pair still stages"
    );
    assert!(
        out.globals.contains_key("__const_stage"),
        "one shared staging buffer must be placed"
    );
}

#[test]
fn value_select_result_gets_a_local_slot() {
    // A non-pointer (value) select copies the selected operand into its dst
    // like any other value (Lane C, #287: the `Inst::Select(s) if !s.ptr`
    // arm of def_width). It must allocate a RAM slot for %s; folding it to
    // no-slot (dropping the value-select arm) leaves %s unallocated.
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %c = icmp eq i8 1, 1\n\
             %s = select i1 %c, i8 1, i8 2\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert!(
        out.locals.contains_key("main::s"),
        "value select %s must get a local slot"
    );
}

#[test]
fn const_300_byte_table_gets_no_ram_address_and_layout_unchanged() {
    // A 300-byte const table (u16 size) gets NO RAM address (its bytes live
    // in flash) but is recorded in const_globals; the surrounding RAM globals
    // keep their layout exactly as if the const didn't exist.
    let src = "global a i8\n\
         global after i8\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n";
    let mut m = parse(src);
    m.globals.insert(
        1, // between `a` and `after`
        ir::Global {
            name: "table".into(),
            ty: ir::Ty::I8,
            is_const: true,
            size: 300, // [300 x i8] const table (u16 size)
            bytes: vec![0u8; 300],
            addr: None,
            refs: Vec::new(),
        },
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // const table: no RAM address, but listed in const_globals.
    assert!(
        !out.globals.contains_key("table"),
        "const table must not get a RAM address"
    );
    assert!(
        out.const_globals.contains("table"),
        "const table must be in const_globals"
    );
    // RAM layout unchanged: a at 0x20, after at 0x21 (const skipped).
    assert_eq!(out.globals["a"], 0x20);
    assert_eq!(out.globals["after"], 0x21);
}

#[test]
fn alloca_byval_and_sret_params_get_full_widths_params_first() {
    // A frame carrying a 4-byte alloca, a 4-byte byval param, and a 2-byte
    // sret param must size each slot to its full width (params first, then
    // the alloca), with no overlap. No globals: the root frame starts at
    // 0x20 (scratch/retval live in fixed common RAM, not after the globals).
    let m = parse(
        "fn f(void) (p=byval4, r=sret)\n\
           block entry:\n\
             %buf = alloca 4\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // Params come first, in declaration order: p (byval, 4 bytes) at 0x20,
    // r (sret, 2 bytes) right after p's 4 bytes at 0x24.
    assert_eq!(out.locals["f::p"], 0x20);
    assert_eq!(out.locals["f::r"], 0x24);
    // The alloca lands after the params, at its full 4-byte size.
    assert_eq!(out.locals["f::buf"], 0x26);
    // No overlap: each slot strictly follows the previous slot's end.
    assert!(
        out.locals["f::r"] >= out.locals["f::p"] + 4,
        "sret overlaps byval param"
    );
    assert!(
        out.locals["f::buf"] >= out.locals["f::r"] + 2,
        "alloca overlaps sret param"
    );
    assert_eq!(out.total_bank0, 4 + 2 + 4);
}

#[test]
fn i32_param_and_def_get_four_bytes() {
    // Milestone 12: alloc is ty.bytes()-driven — an i32 scalar param and an
    // i32 def must each consume a full 4-byte slot (no intra-frame alignment,
    // exactly like the i16 slots in params_are_frame_locals_too).
    let m = parse(
        "const sink i32\nfn f(i32) (p=i32)\n\
           block entry:\n\
             %q = add i32 %p, 1\n\
             %r = add i32 %q, 2\n\
             %s = add i32 %p, %r\n\
             store i32 %s, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // p i32 at 0x20, q at 0x24, r at 0x28 (contiguous 4-byte slots); s
    // reuses q's slot (q is dead once r and s are computed). The heat
    // reorder (epic-cc#535) only fires on a device with an access window,
    // and PIC16F877A has none, so the per-name addresses stay pinned here.
    assert_eq!(out.total_bank0, 4 + 4 + 4);
    assert_eq!(out.locals["f::p"], 0x20);
    assert_eq!(out.locals["f::q"], 0x24);
    assert_eq!(out.locals["f::r"], 0x28);
    assert_eq!(out.locals["f::s"], 0x24);
}

#[test]
#[should_panic(expected = "0x1EF")]
fn frame_exceeding_all_banks_panics() {
    // 250 i16 locals = 500 bytes, more than the 320 GPR bytes across all four
    // banks (4 x 80-byte regions, bank 3 at 0x1A0-0x1EF), so allocation
    // panics past 0x1EF.
    let mut src = String::from("const sink i16\nfn main(void) ()\n  block entry:\n");
    for i in 0..250 {
        src.push_str(&format!("    %v{i} = add i16 1, 2\n"));
    }
    for i in 0..250 {
        src.push_str(&format!("    store i16 %v{i}, ptr @sink\n"));
    }
    src.push_str("    ret void\n");
    let m = parse(&src);
    let _ = allocate(&PIC16F877A, &m, "depth 1\n");
}

#[test]
fn layout_is_debug_printable_and_default() {
    let l: AllocLayout = Default::default();
    assert!(l.globals.is_empty() && l.locals.is_empty() && l.total_bank0 == 0);
    let _ = format!("{l:?}");
}

/// main's context (main -> m1 -> m2, one i8 local each) occupies
/// 0x20..0x23; the ISR root's frame base is AFTER the main context's total,
/// and the `_isr` copies (isr -> m1_isr -> m2_isr) live entirely in that
/// disjoint region — no _isr frame overlaps any main-context frame, so a
/// preempted main's live frames are never clobbered by the ISR context.
#[test]
fn isr_root_region_is_disjoint_from_the_main_context() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %v0 = add i8 1, 2\n\
             call void @m1()\n\
             ret void\n\
         fn m1(void) ()\n\
           block entry:\n\
             %v1 = add i8 1, 2\n\
             call void @m2()\n\
             ret void\n\
         fn m2(void) ()\n\
           block entry:\n\
             %v2 = add i8 1, 2\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %i0 = add i8 1, 2\n\
             call void @m1_isr()\n\
             ret void\n\
         fn m1_isr(void) ()\n\
           block entry:\n\
             %i1 = add i8 1, 2\n\
             call void @m2_isr()\n\
             ret void\n\
         fn m2_isr(void) ()\n\
           block entry:\n\
             %i2 = add i8 1, 2\n\
             ret void\n",
    );
    let out = allocate(
        &PIC16F877A,
        &m,
        "edge main m1\nedge m1 m2\nedge isr m1_isr\nedge m1_isr m2_isr\n",
    );
    // main's context: main at 0x20, m1 at 0x21, m2 at 0x22 (depth_end = 3).
    assert_eq!(out.locals["main::v0"], 0x20);
    assert_eq!(out.locals["m1::v1"], 0x21);
    assert_eq!(out.locals["m2::v2"], 0x22);
    // The ISR root's base is after the main context's total: isr starts at
    // bank0_start + depth_end(main) = 0x23, and the copies follow its chain.
    assert_eq!(out.locals["isr::i0"], 0x23);
    assert_eq!(out.locals["m1_isr::i1"], 0x24);
    assert_eq!(out.locals["m2_isr::i2"], 0x25);
    // Disjointness: no _isr frame overlaps a main-context frame (which
    // occupy 0x20..0x23).
    assert!(
        out.locals["isr::i0"] >= 0x23,
        "isr frame overlaps the main context"
    );
    assert!(
        out.locals["m2_isr::i2"] >= 0x23,
        "m2_isr frame overlaps the main context"
    );
}

/// The ISR root's disjoint base clears the main context's PHYSICAL frame end
/// — not just the virtual depth_end offset. 79 i8 globals fill bank 0 GPR
/// (0x20..0x6E), so the main root frame starts at 0x6F; main's i16 local
/// does not fit the single remaining bank-0 byte and moves wholesale to 0xA0
/// (leaving a 1-byte hole at 0x6F), and the i8 local lands at 0xA2: the
/// physical end is 0xA3, past the virtual depth_end (3 bytes from 0x6F =
/// 0x72). The ISR region must start after the physical end (0xA3), so a
/// preempted main's live spill locals are never overlapped.
#[test]
fn isr_region_clears_the_main_context_physical_frame_end() {
    let mut gsrc = String::new();
    for i in 0..79 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let src = format!(
        "{gsrc}const sink i8\nfn main(void) ()\n\
           block entry:\n\
             %v0 = add i16 1, 2\n\
             %v1 = add i8 3, 4\n\
             store i16 %v0, ptr @sink\n\
             store i8 %v1, ptr @sink\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %i0 = add i8 1, 2\n\
             ret void\n"
    );
    let m = parse(&src);
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // 79 i8 globals: the last at 0x6E, so the root frame starts at 0x6F.
    assert_eq!(out.globals["g78"], 0x6E);
    // main's frame: i16 at 0xA0-0xA1 (moved wholesale past the 0x6F hole),
    // i8 at 0xA2 — true physical end 0xA3.
    assert_eq!(out.locals["main::v0"], 0xA0);
    assert_eq!(out.locals["main::v1"], 0xA2);
    // The ISR base clears the PHYSICAL end (0xA3), not the virtual depth_end
    // offset (0x72, which would land the ISR right on main's live locals).
    assert_eq!(out.locals["isr::i0"], 0xA3);
    assert!(
        out.locals["isr::i0"] > out.locals["main::v1"],
        "isr frame overlaps main's live spill locals"
    );
}

#[test]
fn bank_used_tracks_high_water_per_bank() {
    // 79 i8 globals fill bank 0 GPR (0x20..0x6E); main's i16 local moves
    // wholesale to 0xA0 (bank 1) leaving a 1-byte hole at 0x6F, and its
    // i8 local lands at 0xA2. bank_used[0] = 0x6E - 0x20 + 1 = 79 (the
    // hole at 0x6F is not allocated), bank_used[1] = 0xA3 - 0xA0 = 3,
    // banks 2-3 = 0.
    let mut gsrc = String::new();
    for i in 0..79 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let m = parse(&format!(
        "{gsrc}const sink i8\nfn main(void) ()\n\
               block entry:\n\
                 %v0 = add i16 1, 2\n\
                 %v1 = add i8 3, 4\n\
                 store i16 %v0, ptr @sink\n\
                 store i8 %v1, ptr @sink\n\
                 ret void\n"
    ));
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.bank_used, vec![79, 3, 0, 0]);
    assert_eq!(out.isr_bytes, 0);
}

#[test]
fn bank_used_counts_a_multi_byte_value_in_full() {
    // A single i16 local at 0x20 occupies 0x20..0x22: the high-water END is
    // 0x22, so bank_used[0] = 2, not 1 (tracking the start would undercount
    // by width - 1).
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %v0 = add i16 1, 2\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.bank_used, vec![2, 0, 0, 0]);
}

#[test]
fn isr_bytes_reports_the_disjoint_region_span() {
    // main's context occupies 0x20..0x23 (depth_end 3); the ISR root's
    // base is 0x23 and its chain (isr -> m1_isr -> m2_isr, one i8 local
    // each) ends at 0x26. isr_bytes = 0x26 - 0x23 = 3.
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %v0 = add i8 1, 2\n\
             call void @m1()\n\
             ret void\n\
         fn m1(void) ()\n\
           block entry:\n\
             %v1 = add i8 1, 2\n\
             call void @m2()\n\
             ret void\n\
         fn m2(void) ()\n\
           block entry:\n\
             %v2 = add i8 1, 2\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %i0 = add i8 1, 2\n\
             call void @m1_isr()\n\
             ret void\n\
         fn m1_isr(void) ()\n\
           block entry:\n\
             %i1 = add i8 1, 2\n\
             call void @m2_isr()\n\
             ret void\n\
         fn m2_isr(void) ()\n\
           block entry:\n\
             %i2 = add i8 1, 2\n\
             ret void\n",
    );
    let out = allocate(
        &PIC16F877A,
        &m,
        "edge main m1\nedge m1 m2\nedge isr m1_isr\nedge m1_isr m2_isr\n",
    );
    assert_eq!(out.isr_bytes, 3);
    // has_isr is true: an [isr]-annotated function is present (Lane C, #287:
    // the `!isr_names.is_empty()` field).
    assert!(out.has_isr, "an [isr] function must set has_isr");
    // The ISR region is included in the bank totals: the highest ISR
    // address 0x25 is in bank 0, so bank_used[0] = 0x26 - 0x20 = 6.
    assert_eq!(out.bank_used[0], 6);
}

#[test]
fn a_global_layout_sequential_placement_cannot_fit_succeeds_via_bin_packing() {
    // Three 76-byte globals, one 78-byte global, then one 4-byte global (310
    // bytes total, under the device's 320-byte capacity) — declared in an
    // order where the single sequential cursor abandons a 4-byte leftover in
    // each of the first three banks it uses, then the 78-byte global leaves
    // only 2 bytes in the fourth (last) bank — too little for the trailing
    // 4-byte global, which then has no fifth bank to step into. This is the
    // exact reproduction Task 1's and Task 2's unit tests use in isolation;
    // this test proves the fix through the full public `allocate()` entry
    // point.
    let mut src = String::new();
    for i in 0..5 {
        src.push_str(&format!("global g{i} i8\n"));
    }
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    let sizes = [76u16, 76, 76, 78, 4];
    for i in 0..5 {
        m.globals[i].size = sizes[i];
    }

    // Before this plan, this call panics ("alloc: GPR demand exceeds
    // 0x1EF..."). After Task 3, it must succeed, and every global must be
    // placed within exactly one bank with no two overlapping.
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals.len(), 5);
    let mut spans: Vec<(u16, u16)> = (0..5)
        .map(|i| {
            let name = format!("g{i}");
            let start = out.globals[&name];
            (start, start + sizes[i] - 1)
        })
        .collect();
    for &(start, end) in &spans {
        assert!(
            PIC16F877A
                .ram_banks
                .iter()
                .any(|&(bs, be)| start >= bs && end <= be),
            "global at 0x{start:03X}..=0x{end:03X} does not fit inside a single bank"
        );
    }
    spans.sort();
    for w in spans.windows(2) {
        assert!(
            w[0].1 < w[1].0,
            "overlapping placements: {:?} and {:?}",
            w[0],
            w[1]
        );
    }
}

#[test]
#[should_panic(expected = "no arrangement")]
fn globals_truly_exceeding_total_capacity_still_panic_with_a_clear_message() {
    // 5 x 70-byte globals = 350 bytes > the device's 320-byte total GPR
    // capacity: no arrangement fits, so this must still panic, now with a
    // message naming the real constraint instead of a bare hex address.
    let mut src = String::new();
    for i in 0..5 {
        src.push_str(&format!("global g{i} i8\n"));
    }
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    for i in 0..5 {
        m.globals[i].size = 70;
    }
    let _ = allocate(&PIC16F877A, &m, "depth 1\n");
}

#[test]
#[should_panic(expected = "no arrangement")]
fn a_single_global_larger_than_any_bank_panics_even_under_total_capacity() {
    // One 200-byte global on PIC16F877A (4 banks x 80 bytes = 320 bytes total
    // capacity). Total demand (200 bytes) is well under total capacity (320
    // bytes), so this is not a total-capacity failure — it is issue #7's
    // literal case: no single bank window (80 bytes) is big enough to hold
    // this one global by itself, so neither sequential placement nor
    // largest-first bin-packing can ever place it, no matter what else is
    // (or isn't) declared alongside it.
    let mut src = String::new();
    src.push_str("global g0 i8\n");
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    m.globals[0].size = 200;
    let _ = allocate(&PIC16F877A, &m, "depth 1\n");
}

/// The `__mul_u16` routine module used by the issue-#6 rounding tests: two
/// i16 params (a, b) plus a 14-byte scratch alloca (the legalize-injected
/// shape). The frame is 18 bytes; `main` carries `n` i8 locals (no globals,
/// so the root frame starts at 0x20).
fn routine_module(main_locals: u32) -> ir::Module {
    let mut src = String::from(
        "const sink i8\nfn __mul_u16(i16) (a=i16, b=i16)\n\
           block entry:\n\
             %__scr = alloca 14\n\
             store i16 %a, ptr %__scr\n\
             store i16 %b, ptr %__scr\n\
         fn main(void) ()\n\
           block entry:\n",
    );
    for i in 0..main_locals {
        src.push_str(&format!("    %m{i} = add i8 1, 2\n"));
    }
    for i in 0..main_locals {
        src.push_str(&format!("    store i8 %m{i}, ptr @sink\n"));
    }
    src.push_str("    ret void\n");
    parse(&src)
}

#[test]
fn routine_frame_fitting_bank0_stays_put() {
    // main's frame ends at 0x20 + 62 = 0x5E; __mul_u16's 18-byte frame at
    // 0x5E..0x70 fits entirely inside bank 0 (last byte 0x6F), so the
    // derived base is kept (sibling packing is unaffected).
    let m = routine_module(62);
    let out = allocate(&PIC16F877A, &m, "edge main __mul_u16\n");
    assert_eq!(out.locals["__mul_u16::a"], 0x5E);
    assert_eq!(out.locals["__mul_u16::__scr"], 0x62);
    assert_eq!(
        out.locals["__mul_u16::__scr"] + 14,
        0x70,
        "frame ends exactly at the bank-0 boundary"
    );
}

#[test]
fn routine_frame_straddling_rounds_into_the_next_bank() {
    // main's frame ends at 0x20 + 0x40 = 0x60: __mul_u16's 18-byte frame
    // derived at 0x60 would straddle the bank-0/1 boundary, with params at
    // 0x60/0x62 but the 14-byte scratch hopping to 0xA0 (place_contiguous
    // moves the whole local), leaving the frame split across banks, which is
    // forbidden (skip-sensitive recipe loops, issue #6). The base rounds
    // wholesale to bank 1 (0xA0), so the whole frame sits inside it.
    let m = routine_module(0x40);
    let out = allocate(&PIC16F877A, &m, "edge main __mul_u16\n");
    assert_eq!(
        out.locals["__mul_u16::a"], 0xA0,
        "rounded to bank 1's start"
    );
    assert_eq!(out.locals["__mul_u16::b"], 0xA2);
    assert_eq!(out.locals["__mul_u16::__scr"], 0xA4);
    assert_eq!(
        out.locals["__mul_u16::__scr"] + 13,
        0xB1,
        "whole frame inside bank 1"
    );
}

#[test]
fn routine_rounding_wastes_only_the_partial_bank() {
    // main -> f (74 i8 locals: frame 0x20..0x6A, physical end 0x6A); f calls
    // __udiv_u8 (3 bytes: fits bank 0 at 0x6A, no rounding) and __mul_u16
    // (18 bytes: derived at 0x6A, the 14-byte scratch would hop past the
    // common region into bank 1, so the frame rounds wholesale to 0xA0;
    // only the partial bank-0 tail is wasted).
    let mut src = String::from(
        "const sink i8\nfn __udiv_u8(i8) (num=i8, den=i8)\n\
           block entry:\n\
             %__scr = alloca 4\n\
             store i8 %num, ptr %__scr\n\
             store i8 %den, ptr %__scr\n\
         fn __mul_u16(i16) (a=i16, b=i16)\n\
           block entry:\n\
             %__scr = alloca 14\n\
             store i16 %a, ptr %__scr\n\
             store i16 %b, ptr %__scr\n\
         fn f(void) ()\n\
           block entry:\n",
    );
    for i in 0..74u32 {
        src.push_str(&format!("    %f{i} = add i8 1, 2\n"));
    }
    for i in 0..74u32 {
        src.push_str(&format!("    store i8 %f{i}, ptr @sink\n"));
    }
    src.push_str("    call void @__udiv_u8()\n    call void @__mul_u16()\n    ret void\n");
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let m = parse(&src);
    let out = allocate(
        &PIC16F877A,
        &m,
        "edge main f\nedge f __udiv_u8\nedge f __mul_u16\n",
    );
    // The 3-byte routine packs in the bank-0 tail at f's frame end.
    assert_eq!(out.locals["__udiv_u8::num"], 0x6A);
    // The 18-byte routine would straddle -> rounds to bank 1.
    assert_eq!(out.locals["__mul_u16::a"], 0xA0);
    assert_eq!(out.locals["__mul_u16::__scr"], 0xA4);
    assert_eq!(
        out.locals["__mul_u16::__scr"] + 14,
        0xB2,
        "whole frame inside bank 1"
    );
}

// ---- liveness overlay (epic-cc#172) ----

/// Two i8 defs in the same block, the first dead before the second is
/// defined: they share one slot. The pre-liveness allocator gave each a
/// byte (frame 2); liveness gives frame 1.
#[test]
fn dead_def_reuses_the_slot() {
    let m = parse(
        "fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %b = add i8 3, 4\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::a"], 0x20);
    assert_eq!(out.locals["f::b"], 0x20, "dead a's slot is reused by b");
    assert_eq!(out.total_bank0, 1);
}

/// Two i8 defs both live at the same point (each stored to a const sink)
/// cannot share: the frame is 2 bytes.
#[test]
fn co_live_values_do_not_share() {
    let m = parse(
        "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %b = add i8 3, 4\n\
             store i8 %a, ptr @sink\n\
             store i8 %b, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::a"], 0x20);
    assert_eq!(out.locals["f::b"], 0x21);
    assert_eq!(out.total_bank0, 2);
}

/// A value live across a call (used after it) keeps its slot; a value dead
/// before the call shares with the callee's frame base region only if the
/// liveness says so; here the live value pins the frame at 2 bytes.
#[test]
fn value_live_across_call_pins_the_frame() {
    let m = parse(
        "const sink i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             call void @callee()\n\
             store i8 %a, ptr @sink\n\
             ret void\n\
         fn callee(void) ()\n\
           block entry:\n\
             %c = add i8 5, 6\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "edge main callee\n");
    // main's frame is 1 byte (a is live across the call); callee's base is
    // main's physical end 0x21.
    assert_eq!(out.locals["main::a"], 0x20);
    assert_eq!(out.locals["callee::c"], 0x21);
}

/// A phi destination is live from the earliest predecessor end (isel's
/// copies) through the merge block: two phi destinations of the same merge
/// never share a slot, and a value dead before the merge's copies can.
#[test]
fn phi_destinations_are_live_at_pred_ends() {
    let m = parse(
        "const sink i8\n\
         fn f(i1) (c=i1)\n\
           block entry:\n\
             br i1 %c, label %t, label %f\n\
           block t:\n\
             %x = add i8 1, 2\n\
             br label %m\n\
           block f:\n\
             %y = add i8 3, 4\n\
             br label %m\n\
           block m:\n\
             %p = phi i8 %x t %y f\n\
             store i8 %p, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // x and y are dead by the merge (only the phi reads them, at the pred
    // ends), so they share a slot; p is live from the pred ends through the
    // merge, so it gets its own.
    assert_eq!(out.locals["f::x"], out.locals["f::y"]);
    assert_ne!(out.locals["f::p"], out.locals["f::x"]);
}

/// Phi-edge coalescing (epic-cc#727): one incoming slot dead after the
/// join lets the destination reuse it, so isel's phi copy becomes a
/// self-copy skip instead of a MOVFF.
#[test]
fn phi_dst_coalesces_with_dead_incoming() {
    let m = parse(
        "const sink i8\n\
         fn f(i1) (c=i1)\n\
           block entry:\n\
             %x = add i8 1, 2\n\
             br i1 %c, label %t, label %ff\n\
           block t:\n\
             br label %m\n\
           block ff:\n\
             br label %m\n\
           block m:\n\
             %p = phi i8 %x t %x ff\n\
             store i8 %p, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::x"], out.locals["f::p"]);
}

/// The same diamond with the incoming read after the merge: the source
/// is live past the copy point, so the destination keeps its own slot.
#[test]
fn phi_dst_keeps_slot_when_incoming_live_after() {
    let m = parse(
        "const sink i8\n\
         fn f(i1) (c=i1)\n\
           block entry:\n\
             %x = add i8 1, 2\n\
             br i1 %c, label %t, label %ff\n\
           block t:\n\
             br label %m\n\
           block ff:\n\
             br label %m\n\
           block m:\n\
             %p = phi i8 %x t %x ff\n\
             %q = add i8 %x, %p\n\
             store i8 %q, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(out.locals["f::x"], out.locals["f::p"]);
}

/// Straight-line range copies (epic-cc#739): a freeze source dead after
/// the copy lets the destination share its slot, so isel's per-lane
/// copies become self-copy skips instead of MOVFFs.
#[test]
fn freeze_dst_coalesces_with_dead_source() {
    let m = parse(
        "const sink i16\n\
         fn f(void) ()\n\
           block entry:\n\
             %x = add i16 1, 2\n\
             %d = freeze i16 %x\n\
             store i16 %d, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::x"], out.locals["f::d"]);
}

/// The same freeze with the source read after the copy: the source is
/// live past the copy point, so the destination keeps its own slot.
#[test]
fn freeze_dst_keeps_slot_when_source_live_after() {
    let m = parse(
        "const sink i16\n\
         fn f(void) ()\n\
           block entry:\n\
             %x = add i16 1, 2\n\
             %d = freeze i16 %x\n\
             %q = add i16 %x, %d\n\
             store i16 %q, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(out.locals["f::x"], out.locals["f::d"]);
}

/// A narrowing cast shares the source base for its copy lane: the
/// destination's low byte is the source's low byte once the source dies.
#[test]
fn trunc_dst_shares_dead_source_base() {
    let m = parse(
        "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %x = add i16 1, 2\n\
             %d = trunc i16 %x to i8\n\
             store i8 %d, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::x"], out.locals["f::d"]);
}

/// A widening cast shares the source base the same way: the copy lanes
/// skip and only the fill lane emits.
#[test]
fn zext_dst_shares_dead_source_base() {
    let m = parse(
        "const sink i16\n\
         fn f(void) ()\n\
           block entry:\n\
             %x = add i8 1, 2\n\
             %d = zext i8 %x to i16\n\
             store i16 %d, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::x"], out.locals["f::d"]);
}

/// Bitmask lanes (epic-cc#763): an or-select lane whose accumulator is
/// dead after the lane shares its slot, so isel's establish copy becomes
/// a self-copy skip instead of a MOVFF.
#[test]
fn lane_dst_coalesces_with_dead_accumulator() {
    let m = parse(
        "global g i8\n\
         const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %f = load i8 @g\n\
             %t = icmp eq i8 %f, 1\n\
             %o = or i8 %a, 4\n\
             %d = select i1 %t, i8 %o, i8 %a\n\
             store i8 %d, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::a"], out.locals["f::d"]);
}

/// The same lane with the accumulator read after it: the source is live
/// past the lane point, so the destination keeps its own slot.
#[test]
fn lane_dst_keeps_slot_when_accumulator_live_after() {
    let m = parse(
        "global g i8\n\
         const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %f = load i8 @g\n\
             %t = icmp eq i8 %f, 1\n\
             %o = or i8 %a, 4\n\
             %d = select i1 %t, i8 %o, i8 %a\n\
             %q = add i8 %a, %d\n\
             store i8 %q, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(out.locals["f::a"], out.locals["f::d"]);
}

/// A two-lane chain collapses onto one slot: each lane's dst is the next
/// lane's dead-after accumulator, so both pins fire in definition order.
#[test]
fn lane_chain_collapses_to_one_slot() {
    let m = parse(
        "global g i8\n\
         const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %f = load i8 @g\n\
             %t1 = icmp eq i8 %f, 1\n\
             %o1 = or i8 %a, 4\n\
             %d1 = select i1 %t1, i8 %o1, i8 %a\n\
             %t2 = icmp ne i8 %f, 0\n\
             %o2 = or i8 %d1, 8\n\
             %d2 = select i1 %t2, i8 %o2, i8 %d1\n\
             store i8 %d2, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::a"], out.locals["f::d1"]);
    assert_eq!(out.locals["f::d1"], out.locals["f::d2"]);
}

/// The or-bool tail (`or` over a zexted compare, no select) pins the same
/// way: bit 0 lands destructively on the dead accumulator.
#[test]
fn or_bool_lane_coalesces_with_dead_accumulator() {
    let m = parse(
        "global g i8\n\
         const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 3, 4\n\
             %f = load i8 @g\n\
             %t = icmp ne i8 %f, 0\n\
             %z = zext i1 %t to i8\n\
             %d = or i8 %a, %z\n\
             store i8 %d, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.locals["f::a"], out.locals["f::d"]);
}

/// A lane inside a loop body repeats each iteration, which linear order
/// cannot see: the pin punts even when the accumulator looks dead after.
#[test]
fn lane_pin_punts_on_cycle() {
    let m = parse(
        "global g i8\n\
         const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             br label %top\n\
           block top:\n\
             %a = add i8 1, 2\n\
             %f = load i8 @g\n\
             %t = icmp eq i8 %f, 1\n\
             %o = or i8 %a, 4\n\
             %d = select i1 %t, i8 %o, i8 %a\n\
             store i8 %d, ptr @sink\n\
             br label %top\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(out.locals["f::a"], out.locals["f::d"]);
}

/// A copy inside a loop body repeats each iteration, which linear order
/// cannot see: the merge punts even when the source looks dead after.
#[test]
fn copy_merge_punts_on_cycle() {
    let m = parse(
        "const sink i16\n\
         fn f(void) ()\n\
           block entry:\n\
             br label %top\n\
           block top:\n\
             %x = add i16 1, 2\n\
             %d = freeze i16 %x\n\
             store i16 %d, ptr @sink\n\
             br label %top\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(out.locals["f::x"], out.locals["f::d"]);
}

/// A loop-carried value (use before def in linear order) spans the loop and
/// cannot alias a value it is co-live with: the back-edge phi and the
/// induction value stay in distinct slots.
#[test]
fn loop_carried_values_do_not_alias() {
    let m = parse(
        "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             br label %h\n\
           block h:\n\
             %i = phi i8 0 entry %next h\n\
             %acc = phi i8 0 entry %sum h\n\
             %sum = add i8 %acc, %i\n\
             %next = add i8 %i, 1\n\
             store i8 %sum, ptr @sink\n\
             br i1 1, label %h, label %exit\n\
           block exit:\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    // i, acc, sum, next are all live in the loop header: 4 distinct slots.
    let mut addrs: Vec<u16> = ["i", "acc", "sum", "next"]
        .iter()
        .map(|v| out.locals[&format!("f::{v}")])
        .collect();
    addrs.sort();
    addrs.dedup();
    assert_eq!(addrs.len(), 4, "loop-carried values must not alias");
}

/// The frame layout is deterministic: two identical modules allocate
/// identically.
#[test]
fn liveness_layout_is_deterministic() {
    let src = "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %b = add i8 3, 4\n\
             store i8 %a, ptr @sink\n\
             store i8 %b, ptr @sink\n\
             ret void\n";
    let o1 = allocate(&PIC16F877A, &parse(src), "depth 1\n");
    let o2 = allocate(&PIC16F877A, &parse(src), "depth 1\n");
    assert_eq!(o1.locals, o2.locals);
    assert_eq!(o1.bank_used, o2.bank_used);
}

/// A store through a local pointer reads the pointed-to value: the alloca
/// stays live across the store, so a value live at the same point cannot
/// share its slot (the store would clobber it). The prefixed pointer form
/// (`%__scr`) must be stripped to match the defs keys.
#[test]
fn store_through_local_pointer_keeps_it_live() {
    let m = parse(
        "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %__scr = alloca 2\n\
             %c = add i8 1, 2\n\
             %d = add i8 3, 4\n\
             store i8 %d %__scr\n\
             %e = add i8 %c, 1\n\
             store i8 %e @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(
        out.locals["f::__scr"], out.locals["f::c"],
        "store through __scr clobbers live c"
    );
    // The alloca is a memory object: its slot is reserved for the whole
    // function, so e (dead after its store) still cannot reuse it.
    assert_ne!(out.locals["f::e"], out.locals["f::__scr"]);
}

/// An asm operand reading a local keeps it live: the prefixed operand form
/// (`%x`) must be stripped to match the defs keys, or the value's slot is
/// reused while the asm reads it.
#[test]
fn asm_operand_keeps_the_value_live() {
    let m = parse(
        "const sink i8\n\
         fn f(void) ()\n\
           block entry:\n\
             %x = add i8 1, 2\n\
             %y = add i8 3, 4\n\
             asm \"movf $0, W\" *m %x\n\
             store i8 %y, ptr @sink\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(
        out.locals["f::x"], out.locals["f::y"],
        "asm reads x while y is live"
    );
}

/// A GEP index is re-read by isel at every load/store through the GEP's
/// result pointer (the FSR setup recomputes the address from the index each
/// time), so the index stays live until the last use of the GEP dst. Without
/// the propagation, the index's slot is reused by a later load temp while
/// the FSR setup still reads it.
#[test]
fn gep_index_stays_live_until_last_gep_use() {
    let m = parse(
        "global arr i8\n         fn f(void) ()\n           block entry:\n             %i = and i8 7, 3\n             %p = gep @arr +0 +1*%i\n             store i8 1 %p\n             %q = gep @arr +0 +1*%i\n             %v = load i8 %q\n             %w = add i8 %v, 1\n             store i8 %w @arr\n             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(
        out.locals["f::i"], out.locals["f::v"],
        "the load temp reuses the index slot while the FSR setup reads it"
    );
}

/// An indirect call's `func` register is read by isel at dispatch time
/// (after the args are loaded), so it stays live through the call. Without
/// the use, a later arg temp reuses its slot and clobbers the function
/// pointer before the compare-and-call chain reads it.
#[test]
fn indirect_call_target_stays_live_through_the_call() {
    let m = parse(
        "const sink i8\n         fn f(void) ()\n           block entry:\n             %fp = load i16 @sink\n             %a = add i8 1, 2\n             %b = call i8 %fp(i8 %a) callees g h\n             store i8 %b, ptr @sink\n             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_ne!(
        out.locals["f::fp"], out.locals["f::a"],
        "the arg temp reuses the fp slot while the dispatch reads it"
    );
}

/// Priority regions (epic-cc#346): a post-legalize module with main, a
/// high ISR and a low ISR (each with one local, each calling its own
/// helper copy) allocates three disjoint frame regions (main below the
/// low save area, low frames above it, high frames above the low
/// context) and reports the low save area base.
#[test]
fn priority_regions_are_disjoint_with_low_save() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %v0 = add i8 1, 2\n\
             call void @helper()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             %h0 = add i8 3, 4\n\
             ret void\n\
         fn hi(void) [isr] [irq1] ()\n\
           block entry:\n\
             %i0 = add i8 5, 6\n\
             call void @helper_isr_high()\n\
             ret void\n\
         fn helper_isr_high(void) ()\n\
           block entry:\n\
             %g0 = add i8 7, 8\n\
             ret void\n\
         fn lo(void) [isr] [irq2] ()\n\
           block entry:\n\
             %j0 = add i8 9, 10\n\
             call void @helper_isr()\n\
             ret void\n\
         fn helper_isr(void) ()\n\
           block entry:\n\
             %l0 = add i8 11, 12\n\
             ret void\n",
    );
    let out = allocate(
        &PIC18F4550,
        &m,
        "edge main helper\nedge hi helper_isr_high\nedge lo helper_isr\n",
    );
    // The low save area exists and sits above the main context.
    let s = out
        .isr_low_save
        .expect("priority mode must set isr_low_save");
    let main_max = out.locals["main::v0"].max(out.locals["helper::h0"]);
    assert!(
        s > main_max,
        "low save area must sit above main (s = {s:#X})"
    );
    // Low frames live above the 12-byte save area...
    let lo_min = out.locals["lo::j0"].min(out.locals["helper_isr::l0"]);
    let lo_max = out.locals["lo::j0"].max(out.locals["helper_isr::l0"]);
    assert!(
        lo_min >= s + 12,
        "low frames must clear the save area (lo = {lo_min:#X}, s = {s:#X})"
    );
    // ...and high frames live above the whole low context.
    let hi_min = out.locals["hi::i0"].min(out.locals["helper_isr_high::g0"]);
    assert!(
        hi_min > lo_max,
        "high frames must sit above low frames (hi = {hi_min:#X}, lo = {lo_max:#X})"
    );
}

/// epic-cc#482: PIC18's access bank is the low 0x000-0x05F of RAM, the only
/// window `isel-pic18` addresses without a `MOVLB`. The frame overlay is what
/// direct file-register operands name, so it goes there and the globals move
/// above it, the reverse of the PIC14 order.
#[test]
fn pic18_places_the_frame_overlay_below_the_globals() {
    let m = overlay_module();
    let out = allocate(&PIC18F4550, &m, "edge main a\nedge main b\n");
    // main's i8 takes the GPR start; a and b never co-live, so both take
    // the i16 slot above it (a0 and a1 are dead defs and share it). The
    // overlay therefore ends at 0x13, which is where the only global goes.
    assert_eq!(out.locals["main::m0"], 0x10);
    assert_eq!(out.locals["a::a0"], 0x11);
    assert_eq!(out.locals["b::b0"], 0x11);
    assert_eq!(out.globals["in"], 0x13);
}

/// A global pinned by address inside the span the overlay wants has nowhere
/// to go, so that module keeps the globals-first layout and the frames start
/// above every pinned address.
#[test]
fn pic18_keeps_globals_first_when_a_pinned_global_blocks_the_overlay() {
    let m = parse(
        "global pinned i8 @0x12\n\
         fn main(void) ()\n\
           block entry:\n\
             %m0 = load i8 @pinned\n\
             call void @a()\n\
             ret void\n\
         fn a(void) ()\n\
           block entry:\n\
             %a0 = add i16 1, 2\n\
             %a1 = add i16 3, 4\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main a\n");
    assert_eq!(
        out.globals["pinned"], 0x12,
        "a pinned global keeps its address"
    );
    let frame_low = out.locals.values().copied().min().expect("locals");
    assert!(
        frame_low > 0x12,
        "frames must clear the pinned global (lowest frame byte {frame_low:#X})"
    );
}

/// PIC14 has no access bank, so its layout is untouched: globals from the
/// GPR start, frames above them.
#[test]
fn pic14_keeps_globals_first() {
    let m = overlay_module();
    let out = allocate(&PIC16F877A, &m, "edge main a\nedge main b\n");
    assert_eq!(out.globals["in"], PIC16F877A.gpr_start());
    let frame_low = out.locals.values().copied().min().expect("locals");
    assert!(
        frame_low > out.globals["in"],
        "PIC14 frames still follow the globals"
    );
}

/// A PIC18 whose RAM is two GPR regions with a hole between them
/// (`p18f2450`) has to keep the globals above the overlay's PHYSICAL end,
/// which `place_contiguous` may have lifted across the hole, not above the
/// byte count of its frames.
#[test]
fn pic18_two_region_device_keeps_the_globals_above_the_overlay() {
    let dev = device::resolve("p18f2450").expect("p18f2450 is a known device");
    assert_eq!(dev.ram_banks.len(), 2, "the point of this fixture");
    let m = overlay_module();
    let out = allocate(dev, &m, "edge main a\nedge main b\n");
    assert_eq!(out.locals["main::m0"], dev.gpr_start());
    let frame_top = out.locals.values().copied().max().expect("locals") + 2;
    assert!(
        out.globals["in"] >= frame_top,
        "globals must clear the overlay (in = {:#X}, frame top = {frame_top:#X})",
        out.globals["in"]
    );
}

/// epic-cc#509: a runtime routine's frame must not straddle a PIC18 BSR bank
/// (256 bytes), which is `isel-pic18`'s `operand()` granularity, not the
/// device's single giant `ram_banks` region. A caller whose frame ends just
/// below 0x100 would otherwise put `__add_f32`'s 22 bytes across it.
#[test]
fn pic18_routine_frame_snaps_to_the_next_bsr_bank() {
    // Counts are calibrated for this fixture: main's frame end lands at
    // 0xF1, so `__add_f32`'s derived base is 0xF1 and its 22-byte frame
    // would span 0xF1..0x107, crossing the boundary; it must snap to
    // 0x100. Arg homing (epic-cc#830) does not move this frame: a homed
    // value keeps its caller slot.
    let mut src = String::from("global sink i8\nglobal in float\n");
    src.push_str("fn __add_f32(float) (a=i32, b=i32)\n  block entry:\n    %__scr = alloca 14\n");
    src.push_str("fn main(void) ()\n  block entry:\n");
    for i in 0..216 {
        src.push_str(&format!("    %v{i} = add i8 1, 2\n"));
    }
    for i in 0..216 {
        src.push_str(&format!("    store i8 %v{i}, ptr @sink\n"));
    }
    src.push_str(
        "    %x = load float @in\n    %y = load float @in\n    %r = call float @__add_f32(float %x, float %y)\n    ret void\n",
    );
    let m = parse(&src);
    let out = allocate(&PIC18F4550, &m, "edge main __add_f32\n");
    let a = out.locals["__add_f32::a"];
    let scr = out.locals["__add_f32::__scr"];
    assert_eq!(a, 0x100, "the frame must start on the next BSR bank");
    assert_eq!(out.locals["__add_f32::b"], 0x104);
    assert_eq!(scr, 0x108);
    assert_eq!(a >> 8, (scr + 13) >> 8, "the whole frame in one BSR bank");
}

/// The snap must not walk a frame back down: on a PIC18 whose `ram_banks`
/// region spans many 256-byte banks, "the next bank's start" is the next
/// 0x100 boundary, not the region's own start (which is where the frame
/// already was).
#[test]
fn pic18_routine_frame_does_not_snap_back_to_the_region_start() {
    // k=216 snaps to 0x100, k=240 clears it without a snap (measured).
    for k in [216usize, 240] {
        let mut src = String::from("global sink i8\nglobal in float\n");
        src.push_str(
            "fn __add_f32(float) (a=i32, b=i32)\n  block entry:\n    %__scr = alloca 14\n",
        );
        src.push_str("fn main(void) ()\n  block entry:\n");
        for i in 0..k {
            src.push_str(&format!("    %v{i} = add i8 1, 2\n"));
        }
        for i in 0..k {
            src.push_str(&format!("    store i8 %v{i}, ptr @sink\n"));
        }
        src.push_str(
            "    %x = load float @in\n    %y = load float @in\n    %r = call float @__add_f32(float %x, float %y)\n    ret void\n",
        );
        let m = parse(&src);
        let out = allocate(&PIC18F4550, &m, "edge main __add_f32\n");
        let a = out.locals["__add_f32::a"];
        let scr = out.locals["__add_f32::__scr"];
        assert!(a >= 0x100, "k={k}: the frame must clear 0x100 (a = {a:#X})");
        assert_eq!(
            a >> 8,
            (scr + 13) >> 8,
            "k={k}: the whole frame in one BSR bank"
        );
    }
}

#[test]
fn oversized_global_places_without_overlap_on_pic18() {
    // epic-cc#608: a 259-byte global must survive the largest-first
    // bin-pack without truncating to width 3 (the old `as u8` put the
    // next global inside it for silent RAM overlap). Sizes declared in
    // ll order; the pack may reorder, so assert pairwise disjointness
    // and containment in the 4550's single GPR region instead of exact
    // addresses.
    let mut src = String::new();
    for i in 0..3 {
        src.push_str(&format!("global g{i} i8\n"));
    }
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    let sizes = [259u16, 100, 100];
    for i in 0..3 {
        m.globals[i].size = sizes[i];
    }
    let out = allocate(&PIC18F4550, &m, "");
    assert_eq!(out.globals.len(), 3);
    let mut spans: Vec<(u16, u16)> = (0..3)
        .map(|i| {
            let start = out.globals[&format!("g{i}")];
            (start, start + sizes[i] - 1)
        })
        .collect();
    for &(start, end) in &spans {
        assert!(
            PIC18F4550
                .ram_banks
                .iter()
                .any(|&(bs, be)| start >= bs && end <= be),
            "global at 0x{start:03X}..=0x{end:03X} outside GPR"
        );
    }
    spans.sort();
    for w in spans.windows(2) {
        assert!(
            w[0].1 < w[1].0,
            "overlapping placements: {:?} and {:?}",
            w[0],
            w[1]
        );
    }
}

/// epic-cc#448: whole-program opt can leave an unnamed numeric entry (the
/// `irparse` placeholder) beside a later block an inlined callee named. A
/// label-keyed sort that ranks any name ahead of any number then puts that
/// named block at index 0, which `frame_layout` reserves for the entry, and
/// the frame's slot order changes. The entry label's spelling must not reach
/// placement, so the numeric-entry shape lays out exactly like the same CFG
/// with a named entry.
#[test]
fn numeric_entry_lays_out_like_a_named_entry() {
    let shape = |entry: &str| {
        format!(
            "fn f(i16) (p=i16)\n\
             block {entry}:\n\
               %1 = add i16 %p, 1\n\
               %2 = add i16 %1, 3\n\
               br label %merged.exit\n\
             block merged.exit:\n\
               %3 = add i16 %2, 5\n\
               ret i16 %3\n"
        )
    };
    let numeric = allocate(&PIC16F877A, &parse(&shape("0")), "depth 1\n");
    let named = allocate(&PIC16F877A, &parse(&shape("entry")), "depth 1\n");
    assert_eq!(numeric.locals, named.locals);
}

/// epic-cc#535: a frame that straddles the access window's end gets its
/// hottest slots first, so the bank-free window prefix carries the bytes
/// that would otherwise spend `MOVLB`.
///
/// The frame must be wider than PIC18F4550's 80-byte window (0x010-0x05F)
/// for the reorder to apply at all, so this builds one: every value is read
/// by a final chain sum, which keeps them all live at once and forces each
/// into its own slot. `%v0` additionally feeds many stores, making it the
/// frame's hottest value.
#[test]
fn the_hottest_slot_sits_at_the_frame_base() {
    const N: usize = 45;
    let mut src = String::from("global sink i16\nfn f(i16) (p=i16)\nblock entry:\n");
    for i in 0..N {
        src.push_str(&format!("  %v{i} = add i16 %p, {i}\n"));
    }
    // Feed %v0 to many stores up front; its range already reaches the sum
    // below, so this only raises its heat. Two per value against the
    // parameter's one gives %v0 an unambiguous margin over it.
    for _ in 0..2 * N {
        src.push_str("  store i16 %v0, ptr @sink\n");
    }
    // Chain-sum every value, keeping all N live to this point.
    src.push_str("  %s0 = add i16 %v0, %v1\n");
    for i in 1..N {
        src.push_str(&format!("  %s{i} = add i16 %s{}, %v{i}\n", i - 1));
    }
    src.push_str(&format!("  store i16 %s{}, ptr @sink\n  ret void\n", N - 1));

    let out = allocate(&PIC18F4550, &parse(&src), "depth 1\n");
    // The frame really does straddle, or the reorder is inert and this test
    // proves nothing.
    assert!(
        out.locals.values().any(|&a| a > 0x5F),
        "the frame must reach past the access window end"
    );
    // The hottest value takes the window's first byte; a cold one does not.
    assert_eq!(
        out.locals["f::v0"], 0x10,
        "the hottest value takes the frame base"
    );
    assert!(
        out.locals["f::v43"] > 0x5F,
        "a cold value spills past the window: {:#x}",
        out.locals["f::v43"]
    );
}

/// Caller-computed call args (epic-cc#830): a single-use scalar def lands
/// in the callee's param slot, so the site copy is a self-copy isel skips.
fn homing_module(extra_main: &str) -> ir::Module {
    parse(&format!(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             {extra_main}\
             call void @callee(i8 %v)\n\
             ret void\n"
    ))
}

#[test]
fn single_use_scalar_arg_homes_into_the_callee_param_slot() {
    let out = allocate(&PIC18F4550, &homing_module(""), "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "the def must target the param slot"
    );
}

#[test]
fn trunc_arg_homes_into_the_callee_param_slot() {
    // Trunc lowers as compute-then-store like the other homed defs
    // so the narrowed byte can land in the param slot.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %w = add i16 1, 2\n\
             %v = trunc i16 %w to i8\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "a homed trunc must target the param slot"
    );
}

#[test]
fn sext_arg_homes_into_the_callee_param_slot() {
    // Same shape as trunc: the widened value lands in the param slot.
    let m = parse(
        "global out i16\n\
         fn callee(void) (p=i16)\n\
           block entry:\n\
             store i16 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %b = add i8 1, 2\n\
             %v = sext i8 %b to i16\n\
             call void @callee(i16 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "a homed sext must target the param slot"
    );
}

#[test]
fn multi_use_arg_value_keeps_its_caller_slot() {
    let out = allocate(
        &PIC18F4550,
        &homing_module("store i8 %v, ptr @out\n"),
        "edge main callee\n",
    );
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "a twice-read value cannot live in the param slot"
    );
}

#[test]
fn call_between_def_and_use_blocks_homing() {
    // Any intervening call rejects: a sibling callee's frame shares the
    // param slot's RAM, so the copy must stay.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn other(void) ()\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             call void @other()\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\nedge main other\n");
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "a call between def and call must keep the copy"
    );
}

#[test]
fn isr_reachable_callee_blocks_homing() {
    // The ISR can preempt main between the early write and the call, so a
    // callee the ISR reaches keeps its site copies.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @callee(i8 1)\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\nedge isr callee\n");
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "an ISR-reachable callee must keep its site copies"
    );
}

#[test]
fn width_mismatched_arg_keeps_its_caller_slot() {
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i16)\n\
           block entry:\n\
             store i16 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "a narrow arg cannot cover a wide param slot"
    );
}

#[test]
fn loop_writers_home_into_a_shared_param_slot() {
    // Each writer runs once per outer iteration, in order, so the other's
    // def re-runs before every read. The loop-blind clobber check kept the
    // first writer and rejected the second (epic-cc#917).
    let m = parse(
        "global out i8\n\
         fn put(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             br label %top\n\
           block top:\n\
             %x = add i8 1, 2\n\
             call void @put(i8 %x)\n\
             br label %mid\n\
           block mid:\n\
             %y = add i8 3, 4\n\
             call void @put(i8 %y)\n\
             br i1 1, label %top, label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main put\n");
    let p = out.locals["put::p"];
    assert!(
        p == out.locals["main::y"],
        "the second loop writer must home into the shared param slot"
    );
}

#[test]
fn same_block_intruder_after_the_read_keeps_the_owner_out_of_the_slot() {
    // Soundness pin, also passes on the old check: `%t` is written after
    // `%m` is read in `top`, and the loop re-enters `top` to read `%m`
    // again, so a shared slot would hand that read `%t`'s previous value.
    let m = parse(
        "global out i8\n\
         fn put(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %m = add i8 1, 2\n\
             br label %top\n\
           block top:\n\
             call void @put(i8 %m)\n\
             %t = add i8 3, 4\n\
             call void @put(i8 %t)\n\
             br i1 1, label %top, label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main put\n");
    assert_ne!(
        out.locals["put::p"], out.locals["main::m"],
        "an owner read must not share a slot with an intruder write that reaches it again"
    );
}

#[test]
fn chained_call_result_stays_in_the_retval_region() {
    // A chained call result stays in the retval bytes (epic-cc#738) instead
    // of homing into the param slot: the site copy reads it there, so the
    // chain still costs one copy either way.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             call void @callee(i8 %x)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\nedge main callee\n");
    assert_eq!(
        out.locals["main::x"], 0x000,
        "a chained call result stays in the retval region"
    );
}

#[test]
fn single_use_call_result_homes_into_retval() {
    // The result already lands in the retval bytes, so a single clean read
    // leaves it there and deletes the site copy. (epic-cc#738)
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             store i8 %x, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_eq!(
        out.locals["main::x"], 0x000,
        "a single-use result stays in the retval region"
    );
}

#[test]
fn multi_use_call_result_keeps_its_slot() {
    // Two reads share the value, so only a frame slot survives both.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             store i8 %x, ptr @out\n\
             store i8 %x, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_ne!(
        out.locals["main::x"], 0x000,
        "a twice-read result cannot live in retval"
    );
}

#[test]
fn call_between_result_and_use_rejects_homing() {
    // The second call rewrites the retval bytes before the first result is
    // read; the second result's own window is clean, so only it homes.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn other(i8) ()\n\
           block entry:\n\
             %h = add i8 3, 4\n\
             ret i8 %h\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = call i8 @gen()\n\
             %b = call i8 @other()\n\
             store i8 %a, ptr @out\n\
             store i8 %b, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\nedge main other\n");
    assert_ne!(
        out.locals["main::a"], 0x000,
        "a result read past another call cannot live in retval"
    );
    assert_eq!(
        out.locals["main::b"], 0x000,
        "a result read before the next call stays in retval"
    );
}

#[test]
fn pic14_call_result_never_homes_into_retval() {
    // The homing gate is the PIC18-only fixed retval reservation.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             store i8 %x, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "edge main gen\n");
    assert_ne!(
        out.locals["main::x"], 0x000,
        "PIC14 has no fixed retval region to home into"
    );
}

#[test]
fn wide_call_result_keeps_its_slot() {
    // The value region holds four bytes; an eight-byte result cannot fit.
    let m = parse(
        "global out i64\n\
         fn gen(i64) ()\n\
           block entry:\n\
             ret i64 0\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i64 @gen()\n\
             store i64 %x, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_ne!(
        out.locals["main::x"], 0x000,
        "an i64 result cannot live in the 4-byte retval region"
    );
}

#[test]
fn gep_extended_read_rejects_homing() {
    // The pointer's only raw use is the GEP, but isel re-reads it at every
    // load through the derived address, including past the later call.
    // Without the propagation this looks single-use. (epic-cc#738)
    let m = parse(
        "global arr i8\n\
         fn get(i16) ()\n\
           block entry:\n\
             ret i16 0\n\
         fn other(void) ()\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = call i16 @get()\n\
             %g = gep %p +0\n\
             %v = load i8 %g\n\
             call void @other()\n\
             %w = load i8 %g\n\
             store i8 %w, ptr @arr\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main get\nedge main other\n");
    assert_ne!(
        out.locals["main::p"], 0x000,
        "a GEP address re-read past a call cannot live in retval"
    );
}

#[test]
fn ret_forwarded_call_result_homes_into_retval() {
    // `return call f()`: the value never leaves the region, and isel skips
    // the Ret round trip once the slot maps onto it.
    let m = parse(
        "fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn wrap(i8) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             ret i8 %x\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge wrap gen\n");
    assert_eq!(
        out.locals["wrap::x"], 0x000,
        "a directly returned result stays in the retval region"
    );
}

#[test]
fn cross_block_clean_path_homes_into_retval() {
    // The call dominates the read and no path between them holds a call.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             br i1 %c, label %t, label %f\n\
           block t:\n\
             store i8 %x, ptr @out\n\
             br label %done\n\
           block f:\n\
             br label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_eq!(
        out.locals["main::x"], 0x000,
        "a dominated clean cross-block read stays in retval"
    );
}

#[test]
fn loop_local_result_homes_within_the_iteration() {
    // Each iteration re-runs the call before the read, so the back edge
    // cannot carry a clobber into it.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             br label %loop\n\
           block loop:\n\
             %x = call i8 @gen()\n\
             store i8 %x, ptr @out\n\
             br i1 %c, label %loop, label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_eq!(
        out.locals["main::x"], 0x000,
        "a same-block loop result stays in retval"
    );
}

#[test]
fn asm_between_result_and_use_rejects_homing() {
    // Opaque asm could rewrite the fixed bytes, so any asm in the window
    // rejects, even with no call in it.
    let m = parse(
        "global out i8\n\
         fn gen(i8) ()\n\
           block entry:\n\
             %g = add i8 1, 2\n\
             ret i8 %g\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = call i8 @gen()\n\
             asm \"nop\"\n\
             store i8 %x, ptr @out\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main gen\n");
    assert_ne!(
        out.locals["main::x"], 0x000,
        "asm in the window vetoes retval homing"
    );
}

#[test]
fn cross_block_def_homes_with_a_clean_path() {
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             br i1 %c, label %t, label %f\n\
           block t:\n\
             call void @callee(i8 %v)\n\
             br label %done\n\
           block f:\n\
             br label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "a dominated call with no call between must home"
    );
}

#[test]
fn call_on_the_path_blocks_cross_block_homing() {
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn other(void) ()\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             br label %mid\n\
           block mid:\n\
             call void @other()\n\
             br label %tail\n\
           block tail:\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\nedge main other\n");
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "a call on the def-to-call path must keep the copy"
    );
}

#[test]
fn phi_arg_homes_into_the_callee_param_slot() {
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             br i1 %c, label %t, label %f\n\
           block t:\n\
             %x = add i8 1, 2\n\
             br label %m\n\
           block f:\n\
             %y = add i8 3, 4\n\
             br label %m\n\
           block m:\n\
             %v = phi i8 %x t %y f\n\
             call void @callee(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "a phi feeding one call must target the param slot"
    );
}

#[test]
fn passed_through_param_homes_into_the_callee_param_slot() {
    // Callers refresh a param slot at every call site, so a single-use
    // param homes like a def at entry over a call-free entry path.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn mid(i8) (q=i8)\n\
           block entry:\n\
             call void @callee(i8 %q)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge mid callee\n");
    assert_eq!(
        out.locals["mid::q"], out.locals["callee::p"],
        "a passed-through param must target the param slot"
    );
}

#[test]
fn call_before_the_use_blocks_param_homing() {
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn other(void) ()\n\
           block entry:\n\
             ret void\n\
         fn mid(i8) (q=i8)\n\
           block entry:\n\
             call void @other()\n\
             call void @callee(i8 %q)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge mid callee\nedge mid other\n");
    assert_ne!(
        out.locals["mid::q"], out.locals["callee::p"],
        "an entry-prefix call may clobber the param slot"
    );
}

#[test]
fn pass_through_chains_resolve_to_the_final_param_slot() {
    let m = parse(
        "global out i8\n\
         fn inner(void) (q=i8)\n\
           block entry:\n\
             store i8 %q, ptr @out\n\
             ret void\n\
         fn mid(i8) (p=i8)\n\
           block entry:\n\
             call void @inner(i8 %p)\n\
             ret void\n\
         fn outer(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n\
             call void @mid(i8 %v)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge outer mid\nedge mid inner\n");
    assert_eq!(
        out.locals["outer::v"], out.locals["inner::q"],
        "a chained def must reach the final param slot"
    );
    assert_eq!(
        out.locals["mid::p"], out.locals["inner::q"],
        "a chained param must reach the final param slot"
    );
}

#[test]
fn second_homed_source_in_one_caller_keeps_its_slot() {
    // Two single-use defs homed to one param slot would let the later
    // defining write clobber the earlier call's value on the divergent
    // arm, with both site copies skipped: exactly one may home.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             %b = add i8 3, 4\n\
             br i1 %c, label %t, label %f\n\
           block t:\n\
             call void @callee(i8 %b)\n\
             br label %done\n\
           block f:\n\
             call void @callee(i8 %a)\n\
             br label %done\n\
           block done:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    let ahomed = out.locals["main::a"] == out.locals["callee::p"];
    let bhomed = out.locals["main::b"] == out.locals["callee::p"];
    assert!(
        ahomed != bhomed,
        "exactly one same-caller source may home (a={ahomed}, b={bhomed})"
    );
}

#[test]
fn cross_block_writers_to_one_slot_do_not_both_home() {
    // Linear order is not execution order. Order is [entry, 1, 2, 3, 5, 6]:
    // %a's window is (entry, block 1), %bb's is (block 2, block 3), so a
    // linear-tuple test sees them disjoint. But block 2 dominates block 3
    // and reaches block 1, so entry -> 2 -> 1 clobbers the slot with %bb
    // before block 1's call reads %a. Exactly one site may home.
    let m = parse(
        "global out i8\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             br i1 %c, label %2, label %6\n\
           block 2:\n\
             %bb = add i8 3, 4\n\
             br i1 %c, label %1, label %3\n\
           block 1:\n\
             call void @callee(i8 %a)\n\
             br label %5\n\
           block 3:\n\
             call void @callee(i8 %bb)\n\
             br label %5\n\
           block 6:\n\
             br label %5\n\
           block 5:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main callee\n");
    let a = out.locals["main::a"] == out.locals["callee::p"];
    let bb = out.locals["main::bb"] == out.locals["callee::p"];
    assert!(
        !(a && bb),
        "reachable writers must not both home (a={a}, bb={bb})"
    );
}

#[test]
fn chained_sites_converging_on_one_slot_do_not_both_home() {
    // Two args of one call go to different params of g, but g forwards
    // each straight into h's single param. Immediate targets differ
    // (g::p1, g::p2) while the final address is one slot, so selecting on
    // immediate targets would keep both and let the second def clobber
    // the first read.
    let m = parse(
        "global out i8\n\
         fn h(void) (q=i8)\n\
           block entry:\n\
             store i8 %q, ptr @out\n\
             ret void\n\
         fn g(i1) (c=i1, p1=i8, p2=i8)\n\
           block entry:\n\
             br i1 %c, label %2, label %3\n\
           block 2:\n\
             call void @h(i8 %p1)\n\
             br label %4\n\
           block 3:\n\
             call void @h(i8 %p2)\n\
             br label %4\n\
           block 4:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v1 = add i8 1, 2\n\
             %v2 = add i8 3, 4\n\
             call void @g(i1 1, i8 %v1, i8 %v2)\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main g\nedge g h\n");
    let v1 = out.locals["main::v1"] == out.locals["h::q"];
    let v2 = out.locals["main::v2"] == out.locals["h::q"];
    assert!(
        !(v1 && v2),
        "chained writers to one final slot must not both home (v1={v1}, v2={v2})"
    );
}

#[test]
fn sibling_callee_writers_do_not_both_home() {
    // Two different single-param callees share RAM (sibling frames overlay,
    // first param at offset zero), so a second homed def reaching the
    // first site's call clobbers the shared slot even though the callee
    // names differ.
    let m = parse(
        "global out i8\n\
         fn c1(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn c2(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(i1) (c=i1)\n\
           block entry:\n\
             %a = add i8 1, 2\n\
             br i1 %c, label %2, label %6\n\
           block 2:\n\
             %bb = add i8 3, 4\n\
             br i1 %c, label %1, label %3\n\
           block 1:\n\
             call void @c1(i8 %a)\n\
             br label %5\n\
           block 3:\n\
             call void @c2(i8 %bb)\n\
             br label %5\n\
           block 6:\n\
             br label %5\n\
           block 5:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge main c1\nedge main c2\n");
    let a = out.locals["main::a"] == out.locals["c1::p"];
    let bb = out.locals["main::bb"] == out.locals["c2::p"];
    assert!(
        !(a && bb),
        "sibling-frame writers must not both home (a={a}, bb={bb})"
    );
}

#[test]
fn two_params_homed_to_one_slot_do_not_both_home() {
    // Two single-use params of one function, each forwarded from its own
    // arm of a branch. Both write points are recorded at entry, so they
    // tie: without an inclusive same-block test neither collides and the
    // shared slot is written twice per execution.
    let m = parse(
        "global out i8\n\
         fn g(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn f(i1) (c=i1, p1=i8, p2=i8)\n\
           block entry:\n\
             br i1 %c, label %1, label %2\n\
           block 1:\n\
             call void @g(i8 %p1)\n\
             br label %3\n\
           block 2:\n\
             call void @g(i8 %p2)\n\
             br label %3\n\
           block 3:\n\
             ret void\n",
    );
    let out = allocate(&PIC18F4550, &m, "edge f g\n");
    let p1 = out.locals["f::p1"] == out.locals["g::p"];
    let p2 = out.locals["f::p2"] == out.locals["g::p"];
    assert!(
        !(p1 && p2),
        "tied writes to one slot must not both home (p1={p1}, p2={p2})"
    );
}

#[test]
fn pic14_same_bank_arg_still_homes() {
    // epic-cc#849 prices bank selects, it does not ban homing: the tiny
    // frame sits in one bank, so the retargeted def needs no new BANKSEL
    // and the deleted copy is a pure saving.
    let out = allocate(&PIC16F877A, &homing_module(""), "edge main callee\n");
    assert_eq!(
        out.locals["main::v"], out.locals["callee::p"],
        "a same-bank site must still home on a banked core"
    );
}

#[test]
fn pic14_cross_bank_arg_keeps_its_copy() {
    // epic-cc#849: `%v` is defined first (lowest slot, bank 0) but the
    // fifty live i16s push the callee base into bank 1, so homing would
    // move the deleted copy's bank selects onto the retargeted def.
    let mut src = String::from(
        "global out i8\n\
         global sink i16\n\
         fn callee(void) (p=i8)\n\
           block entry:\n\
             store i8 %p, ptr @out\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = add i8 1, 2\n",
    );
    for n in 0..50 {
        src.push_str(&format!("%t{n} = add i16 {n}, 1\n"));
    }
    src.push_str("call void @callee(i8 %v)\n");
    for n in 0..50 {
        src.push_str(&format!("store i16 %t{n}, ptr @sink\n"));
    }
    src.push_str("ret void\n");
    let out = allocate(&PIC16F877A, &parse(&src), "edge main callee\n");
    assert!(
        out.locals["main::v"] < 0x80,
        "the first slot must stay in bank 0: {:#x}",
        out.locals["main::v"]
    );
    assert!(
        out.locals["callee::p"] >= 0xA0,
        "the pushed callee base must reach bank 1: {:#x}",
        out.locals["callee::p"]
    );
    assert_ne!(
        out.locals["main::v"], out.locals["callee::p"],
        "a cross-bank site must keep the site copy"
    );
}

#[test]
fn pic14_drops_direct_load_slot_but_keeps_bin_slot() {
    // The add shape (epic-cc#875): `%1` reads `@in` in W, so its slot
    // drops, while the `add` result still stages into `%2`'s slot.
    // Globals stay exactly placed: the gated pass never moves them.
    let m = parse(
        "global in i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1, 1\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals["in"], 0x20);
    assert_eq!(out.globals["out"], 0x21);
    assert!(
        !out.locals.contains_key("main::1"),
        "folded load needs no slot: {:?}",
        out.locals
    );
    assert!(
        out.locals.contains_key("main::2"),
        "forwarded bin result keeps its slot: {:?}",
        out.locals
    );
}

#[test]
fn pic14_drops_threaded_and_forwarded_load_slots() {
    // `%1` fans out to two stores (ThreadW) and `%2` forwards into one:
    // neither stages (epic-cc#875).
    let m = parse(
        "global in i8\n\
         global slot i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             store i8 %1 @slot\n\
             store i8 %1 @out\n\
             %2 = load i8 @slot\n\
             store i8 %2 @out\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert!(
        !out.locals.contains_key("main::1"),
        "threaded load needs no slot: {:?}",
        out.locals
    );
    assert!(
        !out.locals.contains_key("main::2"),
        "forwarded load needs no slot: {:?}",
        out.locals
    );
}

#[test]
fn pic14_multi_use_load_keeps_its_slot() {
    // `%1` feeds two binops: no fold applies and the slot stays.
    let m = parse(
        "global in i8\n\
         global out i8\n\
         global out2 i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = add i8 %1, 1\n\
             %3 = add i8 %1, 2\n\
             store i8 %2 @out\n\
             store i8 %3 @out2\n\
             ret void\n",
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert!(
        out.locals.contains_key("main::1"),
        "shared load keeps its slot: {:?}",
        out.locals
    );
}

#[test]
fn pooled_consts_drop_ram_copies_except_mixed_uses() {
    // epic-cc#816: with pooling on, a const used only as a direct
    // call arg keeps no RAM copy; one const used both directly and
    // through a dynamic index keeps its copy. Unflagged, both copy
    // as before.
    let mut m = parse(
        "const solo i8\n\
         const mixed i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@solo)\n\
             call void @f(@mixed)\n\
             %i = add i8 1, 2\n\
             %g = gep @mixed +0 +1*%i\n\
             call void @f(%g)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].bytes = b"solo\0".to_vec();
    m.globals[0].size = 5;
    m.globals[1].bytes = b"mixed\0".to_vec();
    m.globals[1].size = 6;
    let unflagged = allocate(&PIC16F877A, &m, "edge main f\n");
    assert!(
        unflagged.globals.contains_key("solo") && unflagged.globals.contains_key("mixed"),
        "unflagged const call args copy to RAM: {:?}",
        unflagged.globals.keys().collect::<Vec<_>>()
    );
    let pooled = allocate_with_pool(&PIC16F877A, &m, "edge main f\n", true);
    assert!(
        !pooled.globals.contains_key("solo") && pooled.const_globals.contains("solo"),
        "direct-only pooled const keeps no RAM copy: {:?}",
        pooled.globals.keys().collect::<Vec<_>>()
    );
    assert!(
        pooled.globals.contains_key("mixed") && !pooled.const_globals.contains("mixed"),
        "dynamically indexed pooled const keeps its copy: {:?}",
        pooled.globals.keys().collect::<Vec<_>>()
    );
    assert!(
        pooled.address_taken_consts.contains("solo")
            && pooled.address_taken_consts.contains("mixed"),
        "both consts feed the pool either way: {:?}",
        pooled.address_taken_consts
    );
}

#[test]
fn pooled_const_with_chase_phi_use_keeps_its_copy() {
    // epic-cc#816: a const feeding a pointer phi (the LSR chase shape)
    // keeps its RAM copy even though its direct call arg alone would
    // gate: iselcore seeds the phi as an indirect slot read with RAM
    // semantics, and a pooled flash address there would deref flash
    // as RAM. The flash lowering of such reads is epic-cc#811.
    let mut m = parse(
        "const solo i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@solo)\n\
             %p = phi ptr @solo entry @solo entry\n\
             %v = load i8 %p\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].bytes = b"solo\0".to_vec();
    m.globals[0].size = 5;
    let pooled = allocate_with_pool(&PIC16F877A, &m, "edge main f\n", true);
    assert!(
        pooled.globals.contains_key("solo") && !pooled.const_globals.contains("solo"),
        "phi-fed pooled const keeps its copy: {:?}",
        pooled.globals.keys().collect::<Vec<_>>()
    );
}

#[test]
fn pooled_const_select_arm_keeps_its_copy() {
    // epic-cc#816: a const arm of a non-folding pointer select keeps
    // its RAM copy: the select seeds an indirect slot isel derefs
    // with RAM semantics, so a pooled flash address there would read
    // flash as RAM.
    let mut m = parse(
        "const a i8\n\
         global ramg i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %c = icmp eq i8 1, 1\n\
             %s = select i1 %c, ptr @a, ptr @ramg\n\
             %v = load i8 %s\n\
             ret void\n",
    );
    m.globals[0].bytes = b"a\0".to_vec();
    m.globals[0].size = 2;
    let unflagged = allocate(&PIC16F877A, &m, "depth 1\n");
    assert!(
        unflagged.globals.contains_key("a"),
        "select-arm const copies without the flag"
    );
    let pooled = allocate_with_pool(&PIC16F877A, &m, "depth 1\n", true);
    assert!(
        pooled.globals.contains_key("a") && !pooled.const_globals.contains("a"),
        "select-arm pooled const keeps its copy: {:?}",
        pooled.globals.keys().collect::<Vec<_>>()
    );
}
