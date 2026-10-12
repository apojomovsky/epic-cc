//! i1 load/store lowering (epic-cc#304): optimizer-shrunk `bool` traffic
//! must lower as one byte, not panic. Hand-written IR keeps the test
//! deterministic; the C source that produces this shape in practice is
//! the HAL's bool handle fields (the epic-hal 16F1937 blink gate).

use device::PIC16F1937;
use ir::parse;
use isel_pic14e::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn lowers_i1_load_and_store_as_one_byte() {
    // `%1` forwards into its store (epic-cc#902): the store reads `@flag`
    // directly, so no slot stages the byte. `%1` keeps no entry: a lookup
    // would panic, so its absence proves the slot is gone. The copy stays
    // a byte MOVF plus a byte MOVWF, never a bit op.
    let m = parse(
        "global flag i8\nfn main(void) ()\n  block entry:\n    %1 = load i1 @flag\n    store i1 %1 @flag\n    ret void\n",
    );
    let addrs = addrs(&[("flag", 0x20)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W"),
        "the load reads the byte:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x20"),
        "the store writes the byte back:\n{asm}"
    );
    assert!(
        !asm.contains("BCF") && !asm.contains("BSF"),
        "no bit ops on the i1 byte path:\n{asm}"
    );
}

#[test]
fn lowers_an_i1_const_store_as_one_byte() {
    let m = parse(
        "global flag i8\nfn main(void) ()\n  block entry:\n    store i1 1 @flag\n    ret void\n",
    );
    let addrs = addrs(&[("flag", 0x20)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(asm.contains("MOVLW 0x01"), "{asm}");
    assert!(asm.contains("MOVWF 0x20"), "{asm}");
}

#[test]
#[should_panic(expected = "unreachable-body")]
fn live_call_to_unreachable_placeholder_is_rejected() {
    // epic-cc#1104: mirrors the isel gate, a live call to an LLVM-proved
    // dead body fails at compile time, never hangs at runtime.
    let m = parse(
        "fn endpoint(void) [unreachable] ()\n  block entry:\n    br entry\n\
         fn main(void) ()\n  block entry:\n    call void @endpoint()\n    ret void\n",
    );
    let _ = select(&PIC16F1937, &m, &addrs(&[]));
}
