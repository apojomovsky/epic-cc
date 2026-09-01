use alloc::{allocate, map_text, AllocLayout};
use device::PIC16F877A;
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
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
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
    // reuses q's slot (q is dead once r and s are computed).
    assert_eq!(out.locals["f::p"], 0x20);
    assert_eq!(out.locals["f::q"], 0x24);
    assert_eq!(out.locals["f::r"], 0x28);
    assert_eq!(out.locals["f::s"], 0x24);
    assert_eq!(out.total_bank0, 4 + 4 + 4);
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
