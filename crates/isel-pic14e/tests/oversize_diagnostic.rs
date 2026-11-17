//! Oversize placement must report the true total against the device
//! limit (epic-cc#1484), not only the first unplaced function.

use device::PIC16F1937;
use ir::parse;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn build(pads: usize, traffic: usize) -> ir::Module {
    let mut ir = String::new();
    for i in 0..7 {
        for j in 0..8 {
            ir.push_str(&format!("global g{i}_{j} i8\n"));
        }
    }
    ir.push_str("global in i8\nglobal out i8\n");
    ir.push_str("fn main(void) ()\n  block entry:\n");
    ir.push_str("    %acc0 = load i8 @in\n");
    let mut reg = 0u32;
    let mut acc = "%acc0".to_string();
    for blk in 0..7 {
        if blk > 0 {
            ir.push_str(&format!("  block b{blk}:\n"));
        }
        for t in 0..traffic {
            let g = format!("g{}_{}", (blk + t) % 7, (blk * 3 + t) % 8);
            reg += 1;
            let r = format!("%t{reg}");
            ir.push_str(&format!("    {r} = load i8 @{g}\n"));
            reg += 1;
            let r2 = format!("%t{reg}");
            ir.push_str(&format!("    {r2} = add i8 {r}, 1\n"));
            ir.push_str(&format!("    store i8 {r2} @{g}\n"));
        }
        for _ in 0..pads {
            reg += 1;
            let r = format!("%t{reg}");
            ir.push_str(&format!("    {r} = add i8 {acc}, 0\n"));
            acc = r;
        }
        if blk < 6 {
            ir.push_str(&format!("    br b{}\n", blk + 1));
        }
    }
    ir.push_str(&format!("    store i8 {acc} @out\n    ret void\n"));
    parse(&ir)
}

fn numbers_after(msg: &str, marker: &str) -> usize {
    let rest = msg
        .split(marker)
        .nth(1)
        .unwrap_or_else(|| panic!("diagnostic must contain {marker:?}:\n{msg}"));
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("{marker:?} must be followed by a number:\n{msg}"))
}

fn oversize_message(m: &ir::Module) -> String {
    let layout = alloc::allocate(&PIC16F1937, m, "depth 1\n");
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let result = catch_unwind(AssertUnwindSafe(|| {
        isel_pic14e::select_with_locs(&PIC16F1937, m, &addrs, &HashSet::new())
    }));
    let err = result.expect_err("oversize program must not compile");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn oversize_with_multi_chunk_const_reports_total_including_chunk_readers() {
    // A 300-byte const is two chunks; its second chunk reader sits
    // after the table data, so the true total ends past the data tail.
    let mut m = build(400, 8);
    m.globals.push(ir::Global {
        name: "zzbig".into(),
        ty: ir::Ty::I8,
        is_const: true,
        size: 300,
        bytes: (0..300).map(|i| (i % 251) as u8).collect(),
        refs: Vec::new(),
        addr: None,
        internal: false,
    });
    let msg = oversize_message(&m);
    let total = numbers_after(&msg, "program needs ");
    assert_eq!(
        total, 13870,
        "exact total must include chunk readers:\n{msg}"
    );
    assert_eq!(
        numbers_after(&msg, "words in total ("),
        total - 8192,
        "overage must be total minus limit:\n{msg}"
    );
}

#[test]
fn oversize_program_reports_total_against_device_limit() {
    // PIC16F1937 flash is 8192 words; this program is sized past it.
    let m = build(400, 8);
    let layout = alloc::allocate(&PIC16F1937, &m, "depth 1\n");
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let result = catch_unwind(AssertUnwindSafe(|| {
        isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &HashSet::new())
    }));
    let err = result.expect_err("oversize program must not compile");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        msg.contains("device flash is 0x2000 words") || msg.contains("8192"),
        "diagnostic must name the device limit:\n{msg}"
    );
    let total = numbers_after(&msg, "program needs ");
    let over = numbers_after(&msg, "words in total (");
    let before_unplaced = msg
        .split(" words still unplaced")
        .next()
        .unwrap_or_else(|| panic!("diagnostic must report unplaced words:\n{msg}"));
    let unplaced_digits: String = before_unplaced
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let unplaced: usize = unplaced_digits
        .parse()
        .unwrap_or_else(|_| panic!("unplaced count must be a number:\n{msg}"));
    assert!(total > 8192, "total must exceed the limit:\n{msg}");
    assert_eq!(
        over,
        total - 8192,
        "overage must be total minus limit:\n{msg}"
    );
    assert!(
        unplaced > 0 && unplaced <= total,
        "unplaced remainder must be reported:\n{msg}"
    );
}
