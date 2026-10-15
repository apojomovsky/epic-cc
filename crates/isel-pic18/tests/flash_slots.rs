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
#[should_panic(expected = "maybe-flash address")]
fn mixed_phi_load_is_refused() {
    // A phi joining flash and RAM has no single address space: loads
    // through it stay refused (epic-cc#1099).
    let m = with_bytes(
        parse(
            "const c i8\nglobal buf i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %p = phi ptr @c entry @buf entry\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
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
            ("main::p", 0x12),
            ("main::v", 0x14),
        ]),
        None,
    );
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
#[should_panic(expected = "maybe-flash address")]
fn mixed_select_forwarded_to_ram_param_is_refused() {
    // A select joining flash and RAM forwarded to a callee would read
    // data memory at a flash address on one arm: refuse (epic-cc#1099).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\nglobal out i8\n\
             fn use(void) (p=ptr)\n  block entry:\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n\
             fn main(void) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    call void @use(ptr %m)\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("out", 0x12),
            ("main::c", 0x13),
            ("main::m", 0x14),
            ("use::p", 0x16),
            ("use::v", 0x18),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "maybe-flash address")]
fn mixed_select_stored_to_ram_is_refused() {
    // Storing a flash-or-RAM address into a RAM slot leaves no single
    // space for later reads: refuse (epic-cc#1099).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\nglobal g i16\n\
             fn main(void) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    store ptr %m @g\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("g", 0x12),
            ("main::c", 0x14),
            ("main::m", 0x15),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "maybe-flash address")]
fn mixed_select_returned_is_refused() {
    // A returned flash-or-RAM address would read back through the
    // caller's RAM slot: refuse (epic-cc#1099).
    let m = with_bytes(
        parse(
            "const s i8\nglobal buf i8\nglobal flag i8\n\
             fn get(ptr) ()\n  block entry:\n    %c = load i8 @flag\n    %m = select i1 %c, ptr @s, ptr @buf\n    ret ptr %m\n\
             fn main(void) ()\n  block entry:\n    ret void\n",
        ),
        "s",
        b"S",
    );
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("buf", 0x10),
            ("flag", 0x11),
            ("get::c", 0x12),
            ("get::m", 0x13),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "laundered through an integer")]
fn int_laundered_flash_address_is_refused() {
    // epic-cc#1284: a flash-kind slot read as `i16` and `inttoptr`ed back
    // would be dereferenced as RAM at the table address. Refuse.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
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
            ("main::p", 0x15),
            ("main::v", 0x17),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "laundered through an integer")]
fn int_laundered_through_second_global_is_refused() {
    // The integer copy travels through another global and a trunc/zext.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    store i16 %x @h\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
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
            ("main::x", 0x15),
            ("main::y", 0x17),
            ("main::p", 0x19),
            ("main::v", 0x1b),
        ]),
        None,
    );
}

#[test]
#[should_panic(expected = "maybe-flash address")]
fn mixed_global_pointer_load_is_refused() {
    // epic-cc#1284: `const char *g = "lit"; g = buf;` holds a flash
    // address (initializer) on one path and a RAM one on another. The
    // loaded pointer has no single space: refuse the dereference instead
    // of reading RAM at a table address.
    let mut m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal buf i8\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %q = gep @buf +0\n    store ptr %q @g\n    %p = load ptr @g\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
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
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("g", 0x10),
            ("buf", 0x12),
            ("out", 0x13),
            ("main::q", 0x14),
            ("main::p", 0x16),
            ("main::v", 0x18),
        ]),
        None,
    );
}
