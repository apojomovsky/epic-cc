//! Address allocation for globals in the PIC8 pipeline.

use ir::Module;

pub const GLOBAL_START: u8 = 0x20;

pub fn allocate(mut m: Module) -> Module {
    let mut addr = GLOBAL_START;
    for g in &mut m.globals {
        if !g.is_const {
            g.addr = Some(addr);
            addr += 1; // i8 globals; i16 -> +2 (later milestone)
        }
    }
    m
}

pub fn address_map(m: &Module) -> String {
    let mut out = String::new();
    for g in &m.globals {
        if let Some(a) = g.addr {
            out.push_str(&format!("global {} 0x{a:02X}\n", g.name));
        }
    }
    out
}
