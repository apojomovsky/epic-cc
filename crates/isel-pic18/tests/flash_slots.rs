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
#[should_panic(expected = "flash address that passed through integers")]
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
#[should_panic(expected = "flash address that passed through integers")]
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
#[should_panic(expected = "flash address that passed through integers")]
fn int_laundered_through_local_slot_is_refused() {
    // epic-cc#1284 review: the integer copy parks in an alloca.
    let m = with_bytes(
        parse(
            "const c i8\nglobal g i16\nglobal out i8\n\
             fn main(void) ()\n  block entry:\n    %a = alloca 2\n    store ptr @c @g\n    %x = load i16 @g\n    store i16 %x %a\n    %y = load i16 %a\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
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
            ("main::a", 0x13),
            ("main::x", 0x15),
            ("main::y", 0x17),
            ("main::p", 0x19),
            ("main::v", 0x1b),
        ]),
        None,
    );
}

/// Parse `ir`, give the const `c` the bytes `P` and assign addresses to
/// every non-const global and every `%reg =` / `%reg` def of `main`.
fn laundering_asm(ir: &str, mutate: impl FnOnce(&mut ir::Module)) -> String {
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
    select(&PIC18F4550, &m, &map, None)
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
#[should_panic(expected = "flash address that passed through integers")]
fn struct_table_pointer_field_read_as_integer_is_refused() {
    let _ = laundering_asm(
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
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn ptrtoint_of_flash_gep_is_refused() {
    // The IR a `(uintptr_t)"lit"` cast produces: a gep, an integer cast,
    // then inttoptr.
    let _ = laundering_asm(
        "const c i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %q = gep @c +0\n    %x = zext i16 %q to i16\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
}

#[test]
#[should_panic(expected = "flash address that passed through integers")]
fn memcpy_of_tainted_slot_is_propagated() {
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    memcpy @h @g 2\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
    );
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
#[should_panic(expected = "flash address that passed through integers")]
fn address_arithmetic_keeps_the_taint() {
    let _ = laundering_asm(
        "const c i8\nglobal g i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    %y = add i16 %x, 1\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |_| {},
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
#[should_panic(expected = "flash address that passed through integers")]
fn const_table_name_field_read_as_integer_is_refused() {
    let _ = laundering_asm(
        "const c i8\nconst tab i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %x = load i16 @tab\n    %p = inttoptr i16 %x to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        |m| tab_mut(m, true),
    );
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
