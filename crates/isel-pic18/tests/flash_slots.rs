use device::PIC18F4550;
use ir::parse;
use isel_pic18::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Set the initializer bytes of a const global (see `isel_pic18.rs`).
fn with_bytes(mut m: ir::Module, name: &str, bytes: &[u8]) -> ir::Module {
    for g in &mut m.globals {
        if g.name == name {
            g.bytes = bytes.to_vec();
            g.size = bytes.len() as u16;
        }
    }
    m
}

/// Count the instructions `select` emits between the `__start:` label and
/// `call main` (see `isel_pic18.rs`).
fn start_steps(asm: &str) -> usize {
    let window = asm
        .split("__start:")
        .nth(1)
        .expect("__start label")
        .split("    call main")
        .next()
        .expect("__start must call main");
    let mut steps = 1; // the call itself
    let mut zero_run = false; // the MOVLW after an LFSR carries the take
    for line in window.lines() {
        let line = line.trim();
        if line.starts_with("LFSR") {
            zero_run = true;
            steps += 1;
        } else if let Some(take) = line.strip_prefix("MOVLW 0x") {
            if zero_run {
                // the MOVLW itself plus the clear-loop body
                steps += 3 * usize::from(u8::from_str_radix(take, 16).unwrap());
                zero_run = false;
            } else {
                steps += 1;
            }
        } else if line.starts_with("MOVWF") || line.starts_with("MOVLB") {
            steps += 1;
        }
    }
    steps
}

/// Advance a fresh simulator through `__start`'s clear/init window (see
/// `isel_pic18.rs`).
fn step_past_start(p: &mut pic14_sim::Pic18, steps: usize) {
    assert!(!p.halted());
    for _ in 0..steps {
        p.step();
    }
    assert!(!p.halted(), "__start window must not halt the machine");
}

#[test]
fn flash_slot_store_loads_back_through_tblrd() {
    // epic-cc#1119: a ROM address stored into a flash-kind global slot
    // lowers whole (`LOW`/`HIGH`), and the load back seeds `TBLPTR`
    // from the slot bytes instead of reading RAM at the address.
    let m = with_bytes(
        parse(
            "const msg i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @msg @g\n    %p = load ptr @g\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "msg",
        b"PQ",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("out", 0x12),
            ("main::p", 0x13),
            ("main::v", 0x15),
        ]),
        None,
    );
    assert!(
        asm.contains("MOVLW LOW(msg)"),
        "store must materialize the table address:\n{asm}"
    );
    assert!(asm.contains("TBLRD*"), "load-back must read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x12], b'P', "load-back must read the table");
}

#[test]
fn flash_slot_store_with_runtime_index_seeds_tblptr() {
    // epic-cc#1119's shape: `g = "lit" + n` with a runtime `n`. The
    // store seeds `TBLPTR` from the table plus the index and keeps its
    // low bytes; the load back adds nothing further.
    let m = with_bytes(
        parse(
            "const msg i8\nglobal idx i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %i = load i8 @idx\n    %p = gep @msg +0 +1*%i\n    store ptr %p @g\n    %q = load ptr @g\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n",
        ),
        "msg",
        b"PQ",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("idx", 0x10),
            ("g", 0x11),
            ("out", 0x13),
            ("main::i", 0x14),
            ("main::q", 0x15),
            ("main::v", 0x17),
        ]),
        None,
    );
    assert!(
        asm.contains("TBLRD*"),
        "dynamic store must seed TBLPTR:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x10] = 1;
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x13], b'Q', "msg[1] must read back");
}

#[test]
fn pure_flash_param_walks_through_tblrd() {
    // epic-cc#1092 at unit scale: a callee walking its pointer param
    // reads program memory when every call site passes flash. No RAM
    // copy exists, so the slot seed is the only sound lowering.
    let m = with_bytes(
        parse(
            "const c i8\nglobal out i8\n\
             fn walk(void) (p=ptr)\n  block entry:\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n\
             fn main(void) ()\n  block entry:\n    call void @walk(ptr @c)\n    ret void\n",
        ),
        "c",
        b"AB",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("out", 0x10), ("walk::p", 0x11), ("walk::v", 0x13)]),
        None,
    );
    assert!(
        asm.contains("MOVLW LOW(c)"),
        "call must pass the table address:\n{asm}"
    );
    assert!(asm.contains("TBLRD*"), "callee must read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x10], b'A', "callee must read the table");
}

#[test]
fn flash_phi_forwarded_to_callee_reads_flash() {
    // A phi over two flash consts forwarded to a callee: the phi slot
    // holds table addresses on both arms, and the callee walks them
    // through TBLPTR (epic-cc#1119 fixes a silent INDF0 misread here).
    let m = with_bytes(
        with_bytes(
            parse(
                "const a i8\nconst b i8\nglobal c i8\nglobal out i8\n\
                 fn deref(void) (p=ptr)\n  block entry:\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n\
                 fn main(void) ()\n  block entry:\n    %l = load i8 @c\n    br i1 %l t f\n  block t:\n    br merge\n  block f:\n    br merge\n  block merge:\n    %p = phi ptr @a t @b f\n    call void @deref(ptr %p)\n    ret void\n",
            ),
            "a",
            b"A",
        ),
        "b",
        b"B",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("c", 0x10),
            ("out", 0x11),
            ("main::l", 0x12),
            ("main::p", 0x13),
            ("deref::p", 0x15),
            ("deref::v", 0x17),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "callee must read flash:\n{asm}");
    for (c, want) in [(0u8, b'B'), (1u8, b'A')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x11], want, "arm c={c} must read its table");
    }
}

#[test]
#[should_panic(expected = "store of a ROM address into RAM")]
fn mixed_global_slot_store_is_refused() {
    // A global also written with a RAM address is no flash-kind slot:
    // storing a ROM address into it stays refused (epic-cc#1101).
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %q = gep @buf +0\n    store ptr %q @g\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[("g", 0x10), ("buf", 0x12), ("main::q", 0x13)]),
        None,
    );
}

#[test]
fn mixed_phi_load_reads_both_spaces() {
    // A phi joining a table address and a RAM address carries its space
    // in the tag: each incoming edge writes address bytes plus the tag,
    // and the deref dispatches on it (epic-cc#1306).
    let m = with_bytes(
        parse(
            "const c i8\nglobal buf i8\nglobal flag i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %f = load i8 @flag\n    br i1 %f t f\n  block t:\n    br merge\n  block f:\n    br merge\n  block merge:\n    %p = phi ptr @c t @buf f\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("out", 0x12),
            ("main::f", 0x13),
            ("main::p", 0x14),
            ("main::v", 0x17),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, want) in [(0u8, b'B'), (1u8, b'P')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x11] = f;
        p.ram_mut()[0x10] = b'B';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x12], want, "arm f={f} must read its space");
    }
}

#[test]
#[should_panic(expected = "cannot pass flash address")]
fn flash_arg_to_ram_param_is_refused() {
    // A flash address bound for a RAM-kind param (mixed call sites here)
    // would read back through FSR: refuse (epic-cc#1119).
    let m = with_bytes(
        parse(
            "const c i8\nglobal buf i8\nglobal out i8\n\
             fn use(void) (p=ptr)\n  block entry:\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n\
             fn main(void) ()\n  block entry:\n    call void @use(ptr @c)\n    call void @use(ptr @buf)\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("out", 0x11),
            ("use::p", 0x12),
            ("use::v", 0x14),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "cannot return flash address")]
fn flash_address_return_is_refused() {
    // A returned flash address would read back through the caller's RAM
    // slot: refuse (alloc copies returned consts to RAM, epic-cc#1119).
    let m = with_bytes(
        parse(
            "const c i8\nglobal out i8\n\
             fn get(ptr) ()\n  block entry:\n    ret ptr @c\n\
             fn main(void) ()\n  block entry:\n    %r = call ptr @get()\n    %v = load i8 %r\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[("out", 0x10), ("main::r", 0x11), ("main::v", 0x13)]),
        None,
    );
}

#[test]
#[should_panic(expected = "store of a ROM address into RAM")]
fn flash_walk_reg_store_into_ram_is_refused() {
    // A walk-phi value stored into a RAM-kind global would read back
    // through RAM at a flash address: refuse (epic-cc#1119).
    let m = with_bytes(
        parse(
            "const t i8\nglobal pv i16\nglobal buf i8\n\
             fn main(void) ()\n  block entry:\n    br loop\n  block loop:\n    %p = phi ptr %n loop @t entry\n    %q = gep @buf +0\n    store ptr %q @pv\n    store ptr %p @pv\n    %n = gep %p +1\n    br loop\n",
        ),
        "t",
        b"PQ",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("pv", 0x10),
            ("buf", 0x12),
            ("main::p", 0x13),
            ("main::q", 0x15),
            ("main::n", 0x17),
        ]),
        None,
    );
}

#[test]
fn mixed_select_forwarded_to_mixed_param_reads_both_spaces() {
    // A select joining flash and RAM forwarded to a callee: the call
    // passes all three bytes, and the callee derefs through the tag
    // (epic-cc#1306).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\nglobal out i8\n\
             fn use(void) (p=ptr)\n  block entry:\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n\
             fn main(void) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    call void @use(ptr %m)\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("out", 0x12),
            ("main::c", 0x13),
            ("main::m", 0x14),
            ("use::p", 0x17),
            ("use::v", 0x1A),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, want) in [(0u8, b'B'), (1u8, b'S')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x11] = f;
        p.ram_mut()[0x10] = b'B';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x12], want, "arm f={f} must read its space");
    }
}

#[test]
fn mixed_select_stored_to_mixed_global_reads_both_spaces() {
    // A select joining flash and RAM stored into a shared global slot:
    // the store writes address bytes plus the tag, and the load back
    // dispatches on it (epic-cc#1306).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    store ptr %m @g\n    %p = load ptr @g\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("g", 0x12),
            ("out", 0x15),
            ("main::c", 0x16),
            ("main::m", 0x17),
            ("main::p", 0x1A),
            ("main::v", 0x1D),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, want) in [(0u8, b'B'), (1u8, b'S')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x11] = f;
        p.ram_mut()[0x10] = b'B';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x15], want, "arm f={f} must read its space");
    }
}

#[test]
fn mixed_select_returned_reads_both_spaces() {
    // A returned flash-or-RAM address travels fat: the callee writes
    // address bytes plus the tag to the retval region, the caller
    // copies all three into its slot, and the deref dispatches on the
    // tag (epic-cc#1306).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\nglobal out i8\n\
             fn get(ptr) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    ret ptr %m\n\
             fn main(void) ()\n  block entry:\n    %r = call ptr @get()\n    %v = load i8 %r\n    store i8 %v @out\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("out", 0x12),
            ("get::c", 0x13),
            ("get::m", 0x14),
            ("main::r", 0x17),
            ("main::v", 0x1A),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, want) in [(0u8, b'B'), (1u8, b'S')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x11] = f;
        p.ram_mut()[0x10] = b'B';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x12], want, "arm f={f} must read its space");
    }
}

#[test]
fn int_laundered_flash_address_reads_flash() {
    // epic-cc#1346: a flash-kind slot read as `i16` and `inttoptr`ed
    // back carries the table address through the integer domain. The
    // space proof settles the space, so the dereference reads flash.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("out", 0x12),
            ("main::x", 0x13),
            ("main::p", 0x15),
            ("main::v", 0x17),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "must read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[0x12],
        b'P',
        "the laundered address must read the table"
    );
}

#[test]
fn int_laundered_through_second_global_reads_flash() {
    // The integer copy travels through another global and a trunc/zext.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    store i16 %x @h\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("h", 0x12),
            ("out", 0x14),
            ("main::x", 0x15),
            ("main::y", 0x17),
            ("main::p", 0x19),
            ("main::v", 0x1b),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "must read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[0x14],
        b'P',
        "the laundered address must read the table"
    );
}

#[test]
fn mixed_global_pointer_load_reads_both_spaces() {
    // epic-cc#1284: `const char *g = "lit"; g = buf;` holds a flash
    // address (initializer) on one path and a RAM one on another. The
    // global slot carries the tag: the initializer sets flash, the RAM
    // store sets RAM, and the load back dispatches on it (epic-cc#1306).
    let mut m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i8\nglobal flag i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %f = load i8 @flag\n    br i1 %f ram join\n  block ram:\n    %q = gep @buf +0\n    store ptr %q @g\n    br join\n  block join:\n    %p = load ptr @g\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "g" {
            g.bytes = vec![1, 0];
            g.refs = vec![(0, "c".to_string(), 0)];
        }
    }
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x13),
            ("flag", 0x14),
            ("out", 0x15),
            ("main::f", 0x16),
            ("main::q", 0x17),
            ("main::p", 0x19),
            ("main::v", 0x1C),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, want) in [(0u8, b'P'), (1u8, b'B')] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x14] = f;
        p.ram_mut()[0x13] = b'B';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x15], want, "arm f={f} must read its space");
    }
}

#[test]
fn int_laundered_through_local_slot_reads_flash() {
    // epic-cc#1284 review: the integer copy parks in an alloca.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %a = alloca 2\n    store ptr @c @g\n    %x = load i16 @g\n    store i16 %x %a\n    %y = load i16 %a\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("out", 0x12),
            ("main::a", 0x13),
            ("main::x", 0x15),
            ("main::y", 0x17),
            ("main::p", 0x19),
            ("main::v", 0x1b),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "must read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[0x12],
        b'P',
        "the laundered address must read the table"
    );
}

/// Parse `ir`, give the const `c` the bytes `P` and assign addresses to
/// every non-const global and every `%reg =` / `%reg` def of `main`.
fn laundering_asm(ir: &str, mutate: impl FnOnce(&mut ir::Module)) -> String {
    laundering_select(ir, mutate).0
}

/// The assembly and the address map for a laundering program.
fn laundering_select(
    ir: &str,
    mutate: impl FnOnce(&mut ir::Module),
) -> (String, HashMap<String, u16>) {
    let mut m = with_bytes(parse(ir), "c", b"P");
    mutate(&mut m);
    let mut pairs: Vec<(String, u16)> = Vec::new();
    let mut next = 0x20u16;
    let mut func = String::from("main");
    for line in ir.split('\n') {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("fn ") {
            func = rest
                .split(|c: char| c == '(' || c.is_whitespace())
                .next()
                .unwrap()
                .to_string();
            // `fn name(ret) (p=ptr, ...)`: params need slots too.
            if let Some(params) = rest.rsplit_once(") (").map(|x| x.1.trim_end_matches(')')) {
                for prm in params.split(',') {
                    if let Some((n, _)) = prm.trim().split_once('=') {
                        pairs.push((format!("{func}::{n}"), next));
                        next += 8;
                    }
                }
            }
        } else if let Some(rest) = t.strip_prefix("global ") {
            pairs.push((rest.split_whitespace().next().unwrap().to_string(), next));
            next += 8;
        } else if let Some(rest) = t.strip_prefix('%') {
            if let Some((name, _)) = rest.split_once(" =") {
                pairs.push((format!("{func}::{name}"), next));
                next += 8;
            }
        }
    }
    let map: HashMap<String, u16> = pairs.into_iter().collect();
    (select(&PIC18F4550, &m, &map, None), map)
}

/// Run a laundering program in the simulator and return the byte it
/// stored in the `out` global. The laundered address must read the
/// table, so the byte is the table's first byte.
fn laundering_out(ir: &str, mutate: impl FnOnce(&mut ir::Module)) -> u8 {
    let (asm, map) = laundering_select(ir, mutate);
    assert!(asm.contains("TBLRD*"), "must read flash:\n{asm}");
    let out = map["out"];
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    p.ram()[out as usize]
}

#[test]
fn struct_table_integer_field_next_to_flash_pointer_compiles() {
    // epic-cc#1284 review: `{ const char *name; uintptr_t addr; }` must not
    // be refused for dereferencing the integer field.
    let asm = laundering_asm(
        "const c i8\nglobal tab i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %a = gep @tab +2\n    %x = load i16 %a\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| {
            for g in &mut m.globals {
                if g.name == "tab" {
                    g.bytes = vec![1, 0, 0x81, 0x0F];
                    g.size = 4;
                    g.refs = vec![(0, "c".to_string(), 0)];
                }
            }
        },
    );
    assert!(
        asm.contains("LFSR 0"),
        "integer field derefs as RAM:\n{asm}"
    );
}

#[test]
fn struct_table_pointer_field_read_as_integer_reads_flash() {
    // The struct's pointer field holds a flash address, so the integer
    // copy of it carries the address through the integer domain.
    let got = laundering_out(
        "const c i8\nglobal tab i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %a = gep @tab +0\n    %x = load i16 %a\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| {
            for g in &mut m.globals {
                if g.name == "tab" {
                    g.bytes = vec![1, 0, 0x81, 0x0F];
                    g.size = 4;
                    g.refs = vec![(0, "c".to_string(), 0)];
                }
            }
        },
    );
    assert_eq!(got, b'P', "the laundered address must read the table");
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn trunc_spelled_ptrtoint_of_flash_gep_is_refused() {
    // irparse spells `ptrtoint` as a same-width `trunc`, the shape the
    // backend now lowers as a copy (epic-cc#1421). Laundering a flash
    // table address into the integer domain must still be refused at the
    // `inttoptr` back, so the copy cannot bypass the epic-cc#1284 guard.
    let _ = laundering_asm(
        "const c i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %q = gep @c +0\n    %x = trunc i16 %q to i16\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn ptrtoint_of_flash_gep_is_refused() {
    // The IR a `(uintptr_t)"lit"` cast produces: a gep, an integer cast,
    // then inttoptr. The gep is folded away by `resolve_pointers`, so
    // its slot holds no address: the cast copies an unwritten slot, and
    // the value is not a table address (epic-cc#1346).
    let _ = laundering_asm(
        "const c i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %q = gep @c +0\n    %x = zext i16 %q to i16\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn memcpy_of_tainted_slot_reads_flash() {
    // The copy moves the flash address's bytes, so the laundered value
    // is still the table address.
    let got = laundering_out(
        "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    memcpy @h @g 2\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
    assert_eq!(got, b'P', "the laundered address must read the table");
}

#[test]
fn memcpy_of_untainted_offset_compiles() {
    // Only bytes 0..2 of `tab` are a flash pointer; the copy of bytes 2..4
    // stays clean.
    let _ = laundering_asm(
        "const c i8\nglobal tab i8\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %s = gep @tab +2\n    memcpy @h %s 2\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| {
            for g in &mut m.globals {
                if g.name == "tab" {
                    g.bytes = vec![1, 0, 0x81, 0x0F];
                    g.size = 4;
                    g.refs = vec![(0, "c".to_string(), 0)];
                }
            }
        },
    );
}

#[test]
fn address_arithmetic_reads_flash() {
    // A constant offset keeps the address in the flash space: the
    // dereference reads the table one byte further on.
    let got = laundering_out(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %y = add i16 %x, 1\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| {
            m.globals
                .iter_mut()
                .find(|g| g.name == "c")
                .expect("const c")
                .bytes = vec![b'P', b'Q'];
        },
    );
    assert_eq!(
        got, b'Q',
        "the offset address must read the next table byte"
    );
}

#[test]
fn pointer_difference_and_byte_mask_are_not_addresses() {
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal g2 i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    store ptr @c @g2\n    %x = load i16 @g\n    %z = load i16 @g2\n    %d = sub i16 %x, %z\n    %m = and i16 %x, 255\n    %s = add i16 %d, %m\n    %p = inttoptr i16 %s to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

fn tab_mut(m: &mut ir::Module, constant: bool) {
    for g in &mut m.globals {
        if g.name == "tab" {
            g.bytes = vec![1, 0, 0x81, 0x0F, 2, 0, 0x82, 0x0F];
            g.size = 8;
            g.refs = vec![(0, "c".to_string(), 0), (4, "c".to_string(), 0)];
            g.is_const = constant;
        }
    }
}

#[test]
fn indexed_table_address_field_compiles() {
    // `tbl[i].addr` over `{ const char *name; uintptr_t addr; }` (stride 4,
    // offset 2) must compile for a RAM table.
    let _ = laundering_asm(
        "const c i8\nglobal tab i8\nglobal idx i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %i = load i8 @idx\n    %a = gep @tab +2 +4*%i\n    %x = load i16 %a\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, false),
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn indexed_table_name_field_read_as_integer_is_refused() {
    let _ = laundering_asm(
        "const c i8\nglobal tab i8\nglobal idx i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %i = load i8 @idx\n    %a = gep @tab +0 +4*%i\n    %x = load i16 %a\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, false),
    );
}

#[test]
fn const_table_name_field_read_as_integer_reads_flash() {
    // The const table's first two bytes are the address of `c`, so the
    // integer read carries that address through the integer domain.
    let got = laundering_out(
        "const c i8\nconst tab i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %x = load i16 @tab\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, true),
    );
    assert_eq!(got, b'P', "the laundered address must read the table");
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn tainted_store_through_param_pointer_reaches_the_caller_slot() {
    let _ = laundering_asm(
        "const c i8\nglobal s i16\nglobal out i8\n\
         fn put(void) (p=ptr)\n  block entry:\n    %q = gep @c +0\n    %z = zext i16 %q to i16\n    store i16 %z %p\n    ret void\n\
         fn main(void) ()\n  block entry:\n    call void @put(ptr @s)\n    %y = load i16 @s\n    %q = inttoptr i16 %y to ptr\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn untainted_caller_of_int_passthrough_callee_compiles() {
    // epic-cc#1324: one tainted call site must not taint every caller's
    // result. `id` returns its argument; the tainted results die unused
    // (storing one to RAM is its own refusal, epic-cc#1101), so the
    // clean caller's `inttoptr` compiles.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn id(i16) (x=i16)\n  block entry:\n    ret i16 %x\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %t = load i16 @g\n    %r1 = call i16 @id(i16 %t)\n    %r3 = call i16 @id(i16 %r1)\n    %r2 = call i16 @id(i16 0)\n    %p = inttoptr i16 %r2 to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn tainted_result_through_int_passthrough_callee_reads_flash() {
    // The tainted site's own result still carries the address: the
    // per-site result taint must survive the epic-cc#1324 tightening.
    let got = laundering_out(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn id(i16) (x=i16)\n  block entry:\n    ret i16 %x\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %t = load i16 @g\n    %r1 = call i16 @id(i16 %t)\n    %p = inttoptr i16 %r1 to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
    assert_eq!(got, b'P', "the laundered address must read the table");
}

#[test]
fn clean_param_stored_to_global_then_loaded_compiles() {
    // epic-cc#1324: a store of a value that merely depends on a param
    // must not taint memory while no site passes taint. Both callers
    // pass clean values, so the loaded `inttoptr` compiles.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn put(void) (x=i16)\n  block entry:\n    store i16 %x @g\n    ret void\n\
         fn main(void) ()\n  block entry:\n    call void @put(i16 0)\n    call void @put(i16 1)\n    %y = load i16 @g\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn split_address_reassembled_with_or_is_refused() {
    // epic-cc#1299: `and 0xFF`/`lshr 8` split the address into bytes and
    // `shl`/`or` rebuilds it. The pair must re-taint.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %lo = and i16 %x, 255\n    %hi = lshr i16 %x, 8\n    %hi2 = shl i16 %hi, 8\n    %r = or i16 %hi2, %lo\n    %p = inttoptr i16 %r to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn split_address_reassembled_with_add_is_refused() {
    // Same split, reassembled with `add` instead of `or`.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %lo = and i16 %x, 255\n    %hi = lshr i16 %x, 8\n    %hi2 = shl i16 %hi, 8\n    %r = add i16 %hi2, %lo\n    %p = inttoptr i16 %r to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn single_address_byte_dereference_compiles() {
    // One fragment alone cannot name the table address: dereferencing
    // just the low or the high byte stays a RAM read, not a refusal.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %lo = and i16 %x, 255\n    %hi = lshr i16 %x, 8\n    %p = inttoptr i16 %lo to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    %q = inttoptr i16 %hi to ptr\n    %w = load i8 %q\n    store i8 %w @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn split_address_with_noop_on_one_leg_is_refused() {
    // Review on epic-cc#1299: `or %frag, 0` is an identity, so a no-op on
    // one leg must not break the reassembly pair back into clean values.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %lo = and i16 %x, 255\n    %lo2 = or i16 %lo, 0\n    %hi = lshr i16 %x, 8\n    %hi2 = shl i16 %hi, 8\n    %r = or i16 %hi2, %lo2\n    %p = inttoptr i16 %r to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "maybe-flash destination")]
fn memcpy_into_mixed_global_is_refused() {
    // A copy into a maybe-flash slot has no tag source: even with a
    // tagged source the destination check must fire first (epic-cc#1306
    // defers the destination side).
    let mut m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i16\nglobal buf i8\n\
             fn main(void) ()\n  block entry:\n    %b = gep @buf +0\n    store ptr %b @g\n    store ptr %b @h\n    %p = load ptr @g\n    memcpy @h %p 1\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "g" || g.name == "h" {
            g.bytes = vec![1, 0];
            g.refs = vec![(0, "c".to_string(), 0)];
        }
    }
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("h", 0x13),
            ("buf", 0x16),
            ("main::b", 0x17),
            ("main::p", 0x19),
        ]),
        None,
    );
}

#[test]
fn dynamic_walk_over_mixed_base_keeps_base_tag() {
    // A runtime-indexed walk preserves its base's space: storing it
    // into a shared slot must copy the base's tag, not tag 0, so the
    // load back dispatches on the right space for every combination
    // of base space and index (epic-cc#1306).
    let mut m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i16\nglobal flag i8\nglobal idx i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %f = load i8 @flag\n    br i1 %f ram join\n  block ram:\n    %b = gep @buf +0\n    store ptr %b @g\n    br join\n  block join:\n    %p = load ptr @g\n    %i = load i8 @idx\n    %w = gep %p +0 +1*%i\n    store ptr %w @g\n    %q = load ptr @g\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"PQ",
    );
    for g in &mut m.globals {
        if g.name == "g" {
            g.bytes = vec![1, 0];
            g.refs = vec![(0, "c".to_string(), 0)];
        }
    }
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x13),
            ("flag", 0x15),
            ("idx", 0x16),
            ("out", 0x17),
            ("main::f", 0x18),
            ("main::b", 0x19),
            ("main::p", 0x1B),
            ("main::i", 0x1E),
            ("main::q", 0x1F),
            ("main::v", 0x22),
        ]),
        None,
    );
    assert!(asm.contains("TBLRD*"), "flash arm must read flash:\n{asm}");
    for (f, i, want) in [
        (0u8, 0u8, b'P'),
        (0u8, 1u8, b'Q'),
        (1u8, 0u8, b'B'),
        (1u8, 1u8, b'R'),
    ] {
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x15] = f;
        p.ram_mut()[0x16] = i;
        p.ram_mut()[0x13] = b'B';
        p.ram_mut()[0x14] = b'R';
        p.run(500);
        assert!(p.halted(), "program must halt");
        assert_eq!(p.ram()[0x17], want, "arm f={f} i={i} must read its space");
    }
}

#[test]
#[should_panic(expected = "load through maybe-flash address")]
fn int_laundered_flash_joined_with_ram_arm_is_refused() {
    // epic-cc#1346: a select joining a laundered flash address with a
    // RAM address is maybe-flash, so no tag survives and the refusal
    // stands. The flash arm alone must not carry the join.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal buf i16\nglobal flag i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %f = load i8 @flag\n    %m = select i1 %f, ptr @buf, ptr %p\n    %v = load i8 %m\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "load through maybe-flash address")]
fn int_laundered_flash_gep_joined_with_ram_arm_is_refused() {
    // The same join, with a GEP between the laundered address and the
    // join. The GEP derives from the flash pointer, so the join is
    // maybe-flash through the derivation too.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal buf i16\nglobal flag i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %q = gep %p +0\n    %f = load i8 @flag\n    %m = select i1 %f, ptr @buf, ptr %q\n    %v = load i8 %m\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn memcpy_of_dynamic_term_source_is_refused() {
    // epic-cc#1346 review: a memcpy source with a runtime term names no
    // fixed bytes, so the copy's destination is not proven to hold the
    // table address, and the laundered value is not a flash address.
    let _ = laundering_asm(
        "const c i8\nglobal tab i8\nglobal idx i8\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %i = load i8 @idx\n    %s = gep @tab +2 +4*%i\n    memcpy @h %s 2\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, false),
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn memcpy_of_dynamic_term_destination_is_refused() {
    // The same, with the runtime term on the destination side: the copy
    // lands in bytes the proof does not name.
    let _ = laundering_asm(
        "const c i8\nglobal tab i8\nglobal idx i8\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %i = load i8 @idx\n    %d = gep @h +0 +4*%i\n    memcpy %d @tab 2\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, false),
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn call_result_over_a_clobbered_param_is_refused() {
    // epic-cc#1346 review: the callee overwrites its parameter's name,
    // so the returned value is not the incoming address.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal ram i16\nglobal out i8\n\
         fn clobber(i16) (x=i16)\n  block entry:\n    %x = load i16 @ram\n    ret i16 %x\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %t = load i16 @g\n    %r = call i16 @clobber(i16 %t)\n    %p = inttoptr i16 %r to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn int_laundered_flash_deref_beside_a_ram_join_reads_flash() {
    // epic-cc#1346 review: a maybe-flash join withdraws the join's
    // result, not the flash arm it joined. A direct dereference
    // through the arm still reads the table byte.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i16\nglobal flag i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    %f = load i8 @flag\n    %m = select i1 %f, ptr @buf, ptr %p\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x13),
            ("flag", 0x16),
            ("out", 0x17),
            ("main::x", 0x18),
            ("main::p", 0x1A),
            ("main::v", 0x1C),
            ("main::f", 0x1D),
            ("main::m", 0x1E),
        ]),
        None,
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.run(500);
    assert!(p.halted(), "program must halt");
    // The table byte lives only in program memory, so a RAM read
    // would return something else.
    assert_eq!(p.ram()[0x17], b'P', "the arm must read the table");
}

#[test]
fn int_laundered_flash_joined_with_a_ram_reg_arm_keeps_ram_read() {
    // epic-cc#1346 review: the RAM arm of the join is a register, not
    // a global. The join is maybe-flash, so the arm keeps its own
    // space: a load through it reads RAM, never program memory.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal ram i16\nglobal data i8\nglobal flag i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %r = load ptr @ram\n    %f = load i8 @flag\n    %m = select i1 %f, ptr %r, ptr %p\n    %v = load i8 %r\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("ram", 0x13),
            ("data", 0x16),
            ("flag", 0x17),
            ("out", 0x18),
            ("main::x", 0x19),
            ("main::p", 0x1B),
            ("main::r", 0x1D),
            ("main::f", 0x1F),
            ("main::m", 0x20),
            ("main::v", 0x22),
        ]),
        None,
    );
    // The RAM arm is read through FSR, never through TBLRD.
    assert!(
        !asm.contains("TBLRD*"),
        "a RAM arm must not read flash:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // The RAM pointer holds the address of the data byte.
    p.ram_mut()[0x13] = 0x16;
    p.ram_mut()[0x14] = 0x00;
    p.ram_mut()[0x16] = b'R';
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x18], b'R', "the RAM arm must read RAM");
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn call_result_over_a_va_arg_clobbered_param_is_refused() {
    // epic-cc#1346 review: the callee overwrites its parameter's name
    // with a `va_arg` result, so the returned value is not the incoming
    // address.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn clobber(i16) (x=i16)\n  block entry:\n    %ap = alloca 4\n    %x = va_arg ptr %ap i16\n    ret i16 %x\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %t = load i16 @g\n    %r = call i16 @clobber(i16 %t)\n    %p = inttoptr i16 %r to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
fn int_laundered_global_with_a_plain_initializer_reads_ram() {
    // epic-cc#1346 review: the global's own initializer holds a plain
    // value, so its bytes are not an address. The laundered value is
    // the initialized one, and the dereference reads RAM, not flash.
    let mut m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "g" {
            g.bytes = vec![0x40, 0x00];
        }
    }
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("out", 0x12),
            ("main::x", 0x13),
            ("main::p", 0x15),
            ("main::v", 0x17),
        ]),
        None,
    );
    assert!(!asm.contains("TBLRD*"), "must not read flash:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x40] = b'R';
    p.run(500);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x12], b'R', "the laundered value must read RAM");
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_address_minus_from_a_constant_is_refused() {
    // epic-cc#1346 review: `n - address` is not an address, so the
    // subtraction does not keep the space.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %y = sub i16 4, %x\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_global_read_before_its_store_is_refused() {
    // epic-cc#1346 review: the global is read before the only store
    // into it, so the read sees the initializer, not the store. The
    // proof cannot say which write a read observes, so the slot is not
    // definite.
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %t = load i16 @g\n    %y = load i16 @h\n    store i16 %t @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_global_read_on_another_switch_arm_is_refused() {
    // epic-cc#1346 review: the flash write is on one switch arm and
    // the read on another, so the read observes the global's reset
    // value, not the write. The switch's edges are part of the block
    // graph the order check reads.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal v i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %v = load i16 @v\n    \
             switch i16 %v, default %def, cases 0 %c0, 1 %c1\n\
             block c0:\n    store ptr @c @g\n    br join\n\
             block c1:\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %c = load i8 %p\n    store i8 %c @out\n    br join\n\
             block def:\n    store ptr @c @g\n    br join\n\
             block join:\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("v", 0x12),
            ("out", 0x14),
            ("main::v", 0x15),
            ("main::x", 0x17),
            ("main::p", 0x19),
            ("main::c", 0x1B),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_pointer_load_before_its_store_is_refused() {
    // epic-cc#1346 review: the pointer load reads the global's bytes
    // before the only store into it, so it observes the global's reset
    // value, not the store. A pointer load counts as a read of the same
    // bytes a value load does.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %p = load ptr @g\n    store ptr @c @g\n    store ptr %p @h\n    %y = load i16 @h\n    %q = inttoptr i16 %y to ptr\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("h", 0x12),
            ("out", 0x14),
            ("main::p", 0x15),
            ("main::y", 0x17),
            ("main::q", 0x19),
            ("main::v", 0x1b),
        ]),
        None,
    );
}

#[test]
fn int_laundered_call_result_looks_up_its_actual_in_the_caller_reads_ram() {
    // epic-cc#1346 review: the callee's own register holds a flash
    // address while the caller's register of the same name holds a RAM
    // address. The actual lives in the caller's scope, so the result
    // is the caller's value, and the dereference reads RAM.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i16\nglobal h i16\nglobal h2 i16\nglobal out i8\n\
             fn f(i16) (x=i16)\n  block entry:\n    store ptr @c @g\n    %1 = load i16 @g\n    ret i16 %x\n\
             fn main(void) ()\n  block entry:\n    %a = gep @buf +512\n    store ptr %a @h\n    %1 = load i16 @h\n    %r = call i16 @f(i16 %1)\n    %q = gep %r +0\n    memcpy @h2 %q 2\n    %y = load i16 @h2\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x13),
            ("h", 0x16),
            ("h2", 0x19),
            ("out", 0x1C),
            ("f::1", 0x1D),
            ("f::x", 0x1F),
            ("main::1", 0x21),
            ("main::r", 0x23),
            ("main::q", 0x25),
            ("main::y", 0x27),
            ("main::p", 0x29),
            ("main::v", 0x2B),
        ]),
        None,
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // The caller's pointer holds a data address the start-up clear
    // never reaches. That address holds another data address, which
    // the copy carries over and the dereference reads.
    p.ram_mut()[0x213] = 0x40;
    p.ram_mut()[0x214] = 0x00;
    p.ram_mut()[0x40] = b'R';
    p.run(500);
    assert!(p.halted(), "program must halt");
    // The table byte lives only in program memory, so a flash read
    // would return something else.
    assert_eq!(p.ram()[0x1C], b'R', "the caller's address must read RAM");
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_memcpy_before_its_source_store_is_refused() {
    // epic-cc#1346 review: the copy reads the source before the only
    // write into it, so the copy holds the source's reset value, not
    // the flash address. A copy reads the source's bytes, so the
    // source's write must have happened by it.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    memcpy @h @g 2\n    store ptr @c @g\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("h", 0x12),
            ("out", 0x14),
            ("main::y", 0x15),
            ("main::p", 0x17),
            ("main::v", 0x19),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_value_of_a_redefined_name_is_refused() {
    // epic-cc#1346 review: the function defines the name twice, so a
    // use cannot be attributed to one definition. The first load holds
    // the RAM value the program means; the later one reads a const
    // table's address, which would be the flash value.
    let mut m = with_bytes(
        parse(
            "const c i8\nconst tab i8\nglobal ram i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %x = load i16 @ram\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    br next\n\
             block next:\n    %x = load i16 @tab\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "tab" {
            g.bytes = vec![1, 0, 0x81, 0x0F];
            g.size = 4;
            g.refs = vec![(0, "c".to_string(), 0)];
            g.is_const = true;
        }
    }
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("ram", 0x10),
            ("out", 0x13),
            ("main::x", 0x14),
            ("main::p", 0x16),
            ("main::v", 0x18),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_param_overwritten_later_in_the_body_is_refused() {
    // epic-cc#1346 review: the body overwrites the parameter's name
    // after the use, so the use observes the incoming value, not the
    // overwrite. A param's name the body writes is a redefinition.
    let mut m = with_bytes(
        parse(
            "const c i8\nconst tab i8\nglobal out i8\n\
             fn f(i16) (x=i16)\n  block entry:\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    %x = load i16 @tab\n    ret void\n\
             fn main(void) ()\n  block entry:\n    call void @f(i16 16)\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "tab" {
            g.bytes = vec![1, 0, 0x81, 0x0F];
            g.size = 4;
            g.refs = vec![(0, "c".to_string(), 0)];
            g.is_const = true;
        }
    }
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("out", 0x10),
            ("f::x", 0x11),
            ("f::p", 0x13),
            ("f::v", 0x15),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_value_used_before_its_def_is_refused() {
    // epic-cc#1346 review: the use precedes the definition, so the
    // value the use observes is the slot's, not the definition's.
    let mut m = with_bytes(
        parse(
            "const c i8\nconst tab i8\nglobal ram i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    br next\n\
             block next:\n    %y = load i16 @tab\n    ret void\n",
        ),
        "c",
        b"P",
    );
    for g in &mut m.globals {
        if g.name == "tab" {
            g.bytes = vec![1, 0, 0x81, 0x0F];
            g.size = 4;
            g.refs = vec![(0, "c".to_string(), 0)];
            g.is_const = true;
        }
    }
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("ram", 0x10),
            ("out", 0x13),
            ("main::y", 0x14),
            ("main::p", 0x16),
            ("main::v", 0x18),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_value_redefined_by_an_icmp_is_refused() {
    // epic-cc#1346 review: the name is redefined by a compare, so the
    // use observes the compare's result, not the load's value.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %i = load i8 @c\n    %x = icmp eq i8 %i, %i\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("out", 0x12),
            ("main::x", 0x13),
            ("main::i", 0x15),
            ("main::p", 0x17),
            ("main::v", 0x19),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_high_half_of_a_wide_store_is_refused() {
    // epic-cc#1346 review: a wide store spreads the value over bytes
    // that hold something else, so the high half is not the address.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i32\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %w = zext i16 %x to i32\n    %y = add i32 %w, 4194304\n    store i32 %y @h\n    %z = load i16 @h +2\n    %p = inttoptr i16 %z to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("h", 0x13),
            ("out", 0x18),
            ("main::x", 0x19),
            ("main::w", 0x1B),
            ("main::y", 0x1F),
            ("main::z", 0x23),
            ("main::p", 0x25),
            ("main::v", 0x27),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_pair_of_byte_stores_is_refused() {
    // epic-cc#1346 review: two one-byte stores compose a pair whose
    // halves are each definite but whose value is not an address.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal g2 i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    store ptr @c @g2\n    %x = load i16 @g\n    %y = load i16 @g2\n    %a = trunc i16 %x to i8\n    %b = trunc i16 %y to i8\n    store i8 %a @h\n    %h1 = gep @h +1\n    store i8 %b %h1\n    %z = load i16 @h\n    %p = inttoptr i16 %z to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("g2", 0x13),
            ("h", 0x16),
            ("out", 0x19),
            ("main::x", 0x1A),
            ("main::y", 0x1C),
            ("main::a", 0x1E),
            ("main::b", 0x1F),
            ("main::h1", 0x20),
            ("main::z", 0x22),
            ("main::p", 0x24),
            ("main::v", 0x26),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_pair_of_overlapping_stores_is_refused() {
    // epic-cc#1346 review: two two-byte stores overlap, so the pair
    // holds one store's low byte and the other's high byte, which is
    // not an address.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal g2 i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    store ptr @c @g2\n    %x = load i16 @g\n    %y = load i16 @g2\n    store i16 %x @h\n    %h1 = gep @h +1\n    store i16 %y %h1\n    %z = load i16 @h\n    %p = inttoptr i16 %z to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("g2", 0x13),
            ("h", 0x16),
            ("out", 0x19),
            ("main::x", 0x1A),
            ("main::y", 0x1C),
            ("main::h1", 0x1E),
            ("main::z", 0x20),
            ("main::p", 0x22),
            ("main::v", 0x24),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_pair_with_a_byte_write_over_it_is_refused() {
    // The same, with a one-byte write over the high half of a
    // two-byte store.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal g2 i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    store ptr @c @g2\n    %x = load i16 @g\n    %y = load i16 @g2\n    %b = trunc i16 %y to i8\n    store i16 %x @h\n    %h1 = gep @h +1\n    store i8 %b %h1\n    %z = load i16 @h\n    %p = inttoptr i16 %z to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("g2", 0x13),
            ("h", 0x16),
            ("out", 0x19),
            ("main::x", 0x1A),
            ("main::y", 0x1C),
            ("main::b", 0x1E),
            ("main::h1", 0x20),
            ("main::z", 0x22),
            ("main::p", 0x24),
            ("main::v", 0x26),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_pair_copied_from_a_chimera_is_refused() {
    // epic-cc#1346 review: the copy takes the source's bytes, so a
    // source whose two bytes come from two writes must not become a
    // slot that looks whole.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal g2 i16\nglobal h i16\nglobal s i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    store ptr @c @g2\n    %x = load i16 @g\n    %y = load i16 @g2\n    store i16 %x @h\n    %h1 = gep @h +1\n    store i16 %y %h1\n    memcpy @s @h 2\n    %z = load i16 @s\n    %p = inttoptr i16 %z to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("g2", 0x13),
            ("h", 0x16),
            ("s", 0x19),
            ("out", 0x1C),
            ("main::x", 0x1D),
            ("main::y", 0x1F),
            ("main::h1", 0x21),
            ("main::z", 0x23),
            ("main::p", 0x25),
            ("main::v", 0x27),
        ]),
        None,
    );
}

#[test]
fn int_laundered_memcpy_from_a_pointer_value_reads_ram() {
    // epic-cc#1346 review: a memcpy whose source is a pointer value
    // copies the pointee's content, which the proof cannot bound, so
    // the copy names no bytes and the laundered value is not an
    // address. The dereference reads RAM.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %p = load ptr @g\n    memcpy @buf %p 2\n    %y = load i16 @buf\n    %q = inttoptr i16 %y to ptr\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n",
        ),
        "c",
        b"P",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x13),
            ("out", 0x16),
            ("main::p", 0x17),
            ("main::y", 0x19),
            ("main::q", 0x1B),
            ("main::v", 0x1D),
        ]),
        None,
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // The copy moved the table's first two bytes into `buf`, so the
    // laundered value is that content, and the dereference reads RAM
    // at the address the content holds.
    p.ram_mut()[0x50] = b'R';
    p.run(500);
    assert!(p.halted(), "program must halt");
    // The table byte lives only in program memory, so a flash read
    // would return something else.
    assert_eq!(p.ram()[0x16], b'R', "the copied content must read RAM");
}
