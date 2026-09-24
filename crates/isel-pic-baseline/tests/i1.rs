//! epic-cc#559: i1 memory ops in the PIC baseline backend.
//!
//! clang's own -O1 GlobalOpt narrows an internal flag only ever written 0/1
//! down to `global i1` (epic-cc#462), so i1 reaches isel as a memory type
//! and must lower like the one-byte i8 path. The backend used to assert
//! "only i8/i16 loads/stores supported"; PIC14 (epic-cc#465) and PIC18
//! (epic-cc#464) already handled it, and this mirrors those tests.

use device::PIC12F509;
use ir::parse;
use isel_pic_baseline::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn load_and_store_i1_use_the_byte_path() {
    let m = parse("global in i1\nglobal out i1\nfn main(void) ()\n  block entry:\n    %1 = load i1 @in\n    store i1 %1 @out\n    ret void\n");
    let a = addrs(&[("in", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC12F509, &m, &a);
    // The one-byte load must read `in` and stage it in %1's own slot, and
    // the store must write `out`: the whole i1 operation is a single byte
    // move, so these two exact pairs are the contract (an address appearing
    // anywhere in the output would be too weak an assertion).
    assert!(
        asm.contains("MOVF 0x10, W") && asm.contains("MOVWF 0x12"),
        "one-byte load of in into %1:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x11"), "one-byte store to out:\n{asm}");
}

#[test]
fn store_an_i1_constant_writes_the_byte_value() {
    let m = parse(
        "global out i1\nfn main(void) ()\n  block entry:\n    store i1 1 @out\n    ret void\n",
    );
    let a = addrs(&[("out", 0x11)]);
    let asm = select(&PIC12F509, &m, &a);
    // The constant is materialized as the byte 0x01 and stored to `out`,
    // not a wider value or an address.
    assert!(asm.contains("MOVLW 0x01"), "constant byte:\n{asm}");
    assert!(asm.contains("MOVWF 0x11"), "store target:\n{asm}");
}

#[test]
fn alloca_ptr_phi_materializes_the_frame_address_not_its_contents() {
    // A pointer phi over an alloca (the loop-reduce walk shape): the
    // alloca's slot IS the object, so its pointer value is that slot's own
    // frame address, a literal. The backend used to assert here
    // (`cannot take the value of a GEP over Slot("b", false)`), and
    // relaxing the assert without this arm would read the object's first
    // bytes as its address (epic-cc#647's defect, epic-cc#652 for this
    // backend).
    let m = parse(
        "global c i8\nfn main(void) ()\n  block entry:\n    %b = alloca 4\n    %c = load i8 @c\n    br i1 %c t f\n  block t:\n    br merge\n  block f:\n    br merge\n  block merge:\n    %p = phi ptr %b t %b f\n    store i8 7 %p\n    ret void\n",
    );
    let a = addrs(&[
        ("c", 0x07),
        ("main::b", 0x10),
        ("main::c", 0x0E),
        ("main::p", 0x11),
    ]);
    let asm = select(&PIC12F509, &m, &a);
    // The phi edge copies the alloca's frame address 0x10 as a literal,
    // never `MOVF 0x10, W` (which would read the object's first byte).
    assert!(
        asm.contains("MOVLW 0x10"),
        "the alloca pointer value must be its frame address literal:\n{asm}"
    );
    assert!(
        !asm.contains("MOVF 0x10, W"),
        "the alloca's contents must not be read as its address:\n{asm}"
    );
}
