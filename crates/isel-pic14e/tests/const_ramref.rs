//! Const-initializer refs to RAM globals (epic-cc#451): a `ptr @ram`
//! field records a ref, but a RAM global has no assembler label, so a
//! `LOW()/HIGH()` literal dies in pass 2. Ref targets resolving in the
//! alloc address map must materialize their address byte; flash targets
//! keep the link-time label literal (epic-cc#154). Mirrors the pic18
//! coverage for #443/#455.

use asm::assemble_pic14e;
use device::PIC16F1937;
use ir::{parse, Module};
use isel_pic14e::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn with_bytes(mut m: Module, name: &str, bytes: &[u8]) -> Module {
    for g in &mut m.globals {
        if g.name == name {
            g.bytes = bytes.to_vec();
            g.size = bytes.len() as u16;
        }
    }
    m
}

/// Set the ref entries of a const global the way irparse records a
/// `ptr @target` field: one entry per byte of the pointer, at absolute
/// blob offsets. The canonical IR text carries no refs.
fn with_refs(mut m: Module, name: &str, refs: &[(usize, &str)]) -> Module {
    for g in &mut m.globals {
        if g.name == name {
            g.refs = refs.iter().map(|(o, f)| (*o, f.to_string())).collect();
        }
    }
    m
}

#[test]
fn const_to_ram_init_ram_ref_materializes_the_alloc_address() {
    // A const global copied to RAM whose ref names a RAM global: the
    // __start init must write the address bytes numerically (epic-cc#451),
    // not a `MOVLW LOW(...)` label the assembler cannot resolve for a RAM
    // global. cfg at 0x040, arr at 0x210: LOW = 0x10, HIGH = 0x02.
    let m = with_refs(
        with_bytes(
            parse("const cfg i8\nfn main(void) ()\n  block entry:\n    ret void\n"),
            "cfg",
            &[0x00, 0x00],
        ),
        "cfg",
        &[(0, "arr"), (1, "arr")],
    );
    let asm = select(&PIC16F1937, &m, &addrs(&[("cfg", 0x040), ("arr", 0x210)]));
    assert!(
        asm.contains("MOVLW 0x10\n    MOVWF 0x40\n    MOVLW 0x02\n    MOVWF 0x41"),
        "init must write the alloc-address bytes into the RAM copy:\n{asm}"
    );
    assert!(
        !asm.contains("LOW(arr)"),
        "a RAM global has no label to resolve:\n{asm}"
    );
    assemble_pic14e(&asm);
}

#[test]
fn const_table_ram_ref_materializes_the_alloc_address() {
    // The register-map shape landing in a const table: each ref byte
    // materializes its address half (epic-cc#451). holding_regs at 0x210.
    let m = with_refs(
        with_bytes(
            parse("const map i8\nfn main(void) ()\n  block entry:\n    ret void\n"),
            "map",
            &[0x00, 0x00],
        ),
        "map",
        &[(0, "holding_regs"), (1, "holding_regs")],
    );
    let asm = select(
        &PIC16F1937,
        &m,
        // `map` stays out of the address map: a table global lives in
        // flash, and addrs membership is what routes a const global to
        // the RAM-copy init instead.
        &addrs(&[("holding_regs", 0x210)]),
    );
    assert!(
        asm.contains("RETLW 0x10\n    RETLW 0x02"),
        "each ref byte must materialize its address half:\n{asm}"
    );
    assert!(
        !asm.contains("LOW(holding_regs)"),
        "a RAM global has no label to resolve:\n{asm}"
    );
    assemble_pic14e(&asm);
}

#[test]
fn const_table_function_ref_keeps_the_label_literal() {
    // A function-address field is a link-time value: it must stay a
    // label literal the assembler resolves (epic-cc#154). The
    // alloc-address path must not capture it: a function is never in
    // the address map.
    let m = with_refs(
        with_bytes(
            parse(
                "const vt i8\n\
                 fn f0(void) ()\n  block entry:\n    ret void\n\
                 fn main(void) ()\n  block entry:\n    ret void\n",
            ),
            "vt",
            &[0x00, 0x00],
        ),
        "vt",
        &[(0, "f0"), (1, "f0")],
    );
    let asm = select(&PIC16F1937, &m, &addrs(&[]));
    assert!(
        asm.contains("RETLW LOW(f0)\n    RETLW HIGH(f0)"),
        "a function address stays a link-time label:\n{asm}"
    );
    assemble_pic14e(&asm);
}
