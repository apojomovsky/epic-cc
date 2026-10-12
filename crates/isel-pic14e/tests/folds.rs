//! Single-use load folds on PIC14E (epic-cc#902): loads whose byte never
//! stages to an overlay slot. Hand-written IR keeps each test
//! deterministic; `alloc` drops the same slots these tests omit, so a
//! backend lookup of a dropped slot would panic here instead of passing.

use device::PIC16F1937;
use ir::parse;
use isel_pic14e::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn direct_load_feeds_alu_from_the_source_global() {
    // `%1 = load @in` feeds one `add` operand: the op reads `@in`, so
    // `%1` stages nowhere. `%2` keeps its slot: a forwarded `Bin` result
    // still computes into the temp on this core.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::2", 0x26)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    ADDLW 0x01"),
        "the add reads the source global:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x26"),
        "the bin result stages into its slot:\n{asm}"
    );
}

#[test]
fn fanout_load_threads_w_through_both_stores() {
    // `%1 = load @in; store %1 @slot; store %1 @out`: one read into W,
    // then a MOVWF per store. `%1` needs no entry: a lookup would panic,
    // so its absence proves the slot is gone.
    let m = parse("global in i8\nglobal slot i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @slot\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("slot", 0x21), ("out", 0x22)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x21\n    MOVWF 0x22"),
        "one read threading W through both stores:\n{asm}"
    );
    assert_eq!(
        asm.matches("MOVF 0x20, W").count(),
        1,
        "the source reads exactly once:\n{asm}"
    );
}

#[test]
fn forwarded_load_store_reads_the_source_global() {
    // `%1 = load @in; store %1 @slot`: the store reads `@in`, so the
    // copy never stages.
    let m = parse("global in i8\nglobal slot i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @slot\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("slot", 0x21)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x21"),
        "store reads the source global:\n{asm}"
    );
}

#[test]
fn forwarded_bin_result_keeps_its_slot() {
    // `%2 = add` forwards into its store, but this backend has no
    // store-folded destination: the bin computes into its temp as before,
    // so `%2` keeps its entry and the slot stays live.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::2", 0x26)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVWF 0x26"),
        "the bin result stages into its slot:\n{asm}"
    );
}

#[test]
fn or_operand_load_stays_staged() {
    // `Or` has no fold shape here: the load stages even single-use, so
    // its slot entry stays live.
    let m = parse("global f i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @f\n    %2 = or i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[
        ("f", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
    ]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x25"),
        "the or operand stages:\n{asm}"
    );
}

#[test]
fn multi_use_load_keeps_its_slot() {
    // `%1` feeds two binops: no fold applies and the slot stays.
    let m = parse("global in i8\nglobal out i8\nglobal out2 i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    %3 = add i8 %1, 2\n    store i8 %2 @out\n    store i8 %3 @out2\n    ret void\n");
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("out2", 0x22),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::3", 0x27),
    ]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x25"),
        "the shared load stages:\n{asm}"
    );
}
