use device::PIC16F1937;
use ir::{parse, Global, Ty};
use isel_pic14e::const_memcpy_parks_hold;
use std::collections::HashMap;

fn global(name: &str, size: usize) -> Global {
    Global {
        name: name.into(),
        ty: Ty::I8,
        is_const: false,
        size: size as u16,
        bytes: vec![0; size],
        addr: None,
        refs: Vec::new(),
    }
}

fn copy_module(dst_size: usize) -> (ir::Module, HashMap<String, u16>) {
    let mut m = parse("fn main(void) ()\n  block 0:\n    memcpy @dst @src 4\n    ret void\n");
    m.globals = vec![global("dst", dst_size), global("src", 4)];
    let addrs = HashMap::from([("dst".to_string(), 0x20), ("src".to_string(), 0x30)]);
    (m, addrs)
}

#[test]
fn constant_copy_to_direct_destination_parks_no_hold_byte() {
    let (m, addrs) = copy_module(4);
    assert!(!const_memcpy_parks_hold(&PIC16F1937, &m, &addrs));
}

#[test]
fn constant_copy_to_bank_straddling_destination_parks_hold_byte() {
    let (m, _) = copy_module(90);
    let addrs = HashMap::from([("dst".to_string(), 0xA0), ("src".to_string(), 0x20)]);
    assert!(const_memcpy_parks_hold(&PIC16F1937, &m, &addrs));
}
