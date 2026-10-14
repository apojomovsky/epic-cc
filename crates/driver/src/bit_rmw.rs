//! Fuse a volatile single-bit read-modify-write of an SFR byte into one
//! bit store (epic-cc#1281, follow-up of epic-cc#707).
//!
//! `PORTBbits.RB1 = 1` reaches the backend as the three-instruction run
//!
//! ```text
//! %a = load volatile i8, ptr inttoptr (i16 6 to ptr)
//! %b = or i8 %a, 2
//! store volatile i8 %b, ptr inttoptr (i16 6 to ptr)
//! ```
//!
//! which lowers to a byte read-modify-write (`MOVF`, `MOVWF`, `IORLW`,
//! `MOVWF`). `BSF`/`BCF` are the hardware read-modify-write of the same
//! register, one read and one write, so the run is semantics-equal to a
//! single bit op. The rewrite replaces the run with a constant store to a
//! bare-bit entity (the epic-cc#707 `.epic.bit.<addr>.<bit>` global), which
//! isel already lowers to one `BSF`/`BCF` and alloc gives no RAM.
//!
//! The run must be adjacent (no volatile op between the load and the
//! store), both accesses volatile bytes at the same literal file address,
//! the mask a single bit (`or` with one set bit, `and` with one clear bit),
//! and the loaded and computed values unused elsewhere. Byte-wide and
//! multi-bit masks keep the byte path. INDF, PCL and STATUS are excluded:
//! a bit op on them is not the byte write the program spelled. Classic
//! PIC14 only: it is the one core with the bit-entity lowering.
use device::Core;
use ir::{BinOp, Global, Inst, Module, Placed, Store, Ty, Val};
use std::collections::HashMap;

/// Registers whose byte write differs from a bit op (indirect access,
/// program counter, flag-bearing status).
const EXCLUDED: [u16; 3] = [0x00, 0x02, 0x03];

pub fn run(m: &mut Module, core: Core) {
    if !matches!(core, Core::Pic14) {
        return;
    }
    let mut known: HashMap<(u16, u8), String> = HashMap::new();
    for g in &m.globals {
        if let Some(Placed {
            byte,
            bit: Some(bit),
        }) = g.addr
        {
            known.entry((byte, bit)).or_insert_with(|| g.name.clone());
        }
    }
    let mut fresh: Vec<Global> = Vec::new();
    for f in &mut m.funcs {
        let mut uses: HashMap<String, usize> = HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                for r in ir::read_vals(inst) {
                    if !r.is_empty() {
                        *uses.entry(r).or_insert(0) += 1;
                    }
                }
            }
        }
        for b in &mut f.blocks {
            let mut i = 0;
            while i + 2 < b.insts.len() {
                let Some((addr, bit, set)) = match_run(&b.insts[i..i + 3], &uses) else {
                    i += 1;
                    continue;
                };
                let name = known
                    .entry((addr, bit))
                    .or_insert_with(|| {
                        let name = format!("__epic_bit_{addr:X}_{bit}");
                        fresh.push(Global {
                            name: name.clone(),
                            ty: Ty::I8,
                            is_const: false,
                            size: 1,
                            bytes: Vec::new(),
                            refs: Vec::new(),
                            addr: Some(Placed {
                                byte: addr,
                                bit: Some(bit),
                            }),
                            internal: false,
                        });
                        name
                    })
                    .clone();
                let loc = match &b.insts[i + 2] {
                    Inst::Store(s) => s.loc.clone(),
                    _ => unreachable!("match_run ends on a store"),
                };
                let repl = Inst::Store(Store {
                    ty: Ty::I8,
                    val: Val::Const(i64::from(set)),
                    ptr: format!("@{name}"),
                    volatile: true,
                    loc,
                });
                b.insts.splice(i..i + 3, [repl]);
                i += 1;
            }
        }
    }
    m.globals.extend(fresh);
}

/// `(address, bit, set)` when the three instructions are a fusable run.
fn match_run(w: &[Inst], uses: &HashMap<String, usize>) -> Option<(u16, u8, bool)> {
    let (Inst::Load(l), Inst::Bin(q), Inst::Store(s)) = (&w[0], &w[1], &w[2]) else {
        return None;
    };
    if !l.volatile || !s.volatile || l.ptr_ty || l.ty != Ty::I8 || s.ty != Ty::I8 || q.ty != Ty::I8
    {
        return None;
    }
    if l.ptr != s.ptr {
        return None;
    }
    let addr = u16::from_str_radix(l.ptr.strip_prefix("0x")?, 16).ok()?;
    if addr > 0xFF || EXCLUDED.contains(&addr) {
        return None;
    }
    let mask = match (&q.a, &q.b) {
        (Val::Reg(r), Val::Const(k)) | (Val::Const(k), Val::Reg(r)) if *r == l.dst => *k & 0xFF,
        _ => return None,
    };
    if !matches!(&s.val, Val::Reg(r) if *r == q.dst) {
        return None;
    }
    if uses.get(&l.dst) != Some(&1) || uses.get(&q.dst) != Some(&1) {
        return None;
    }
    let (single, set) = match q.op {
        BinOp::Or => (mask, true),
        BinOp::And => (!mask & 0xFF, false),
        _ => return None,
    };
    if single.count_ones() != 1 {
        return None;
    }
    Some((addr, single.trailing_zeros() as u8, set))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(body: &str) -> Module {
        let ll = format!(
            "target triple = \"msp430\"\n\ndefine dso_local void @main() {{\n{body}\n  ret void\n}}\n"
        );
        irparse::parse_ll_opts(&ll, false)
    }

    fn count_stores(m: &Module) -> (usize, usize) {
        let mut bit = 0;
        let mut byte = 0;
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Store(s) = i {
                        if s.ptr.starts_with('@') {
                            bit += 1;
                        } else {
                            byte += 1;
                        }
                    }
                }
            }
        }
        (bit, byte)
    }

    const SET: &str = "  %1 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  %2 = or i8 %1, 2\n  store volatile i8 %2, ptr inttoptr (i16 6 to ptr), align 2";

    #[test]
    fn single_bit_or_and_and_fuse_on_pic14() {
        let mut m = module(&format!(
            "{SET}\n  %3 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  %4 = and i8 %3, -5\n  store volatile i8 %4, ptr inttoptr (i16 6 to ptr), align 2"
        ));
        run(&mut m, Core::Pic14);
        assert_eq!(count_stores(&m), (2, 0));
        let bits: Vec<_> = m.globals.iter().filter_map(|g| g.addr).collect();
        assert!(bits.contains(&Placed {
            byte: 6,
            bit: Some(1)
        }));
        assert!(bits.contains(&Placed {
            byte: 6,
            bit: Some(2)
        }));
    }

    #[test]
    fn other_cores_keep_the_byte_path() {
        let mut m = module(SET);
        run(&mut m, Core::Pic14e);
        assert_eq!(count_stores(&m), (0, 1));
    }

    #[test]
    fn multi_bit_masks_and_reused_values_stay() {
        let mut m = module(
            "  %1 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  %2 = or i8 %1, 6\n  store volatile i8 %2, ptr inttoptr (i16 6 to ptr), align 2",
        );
        run(&mut m, Core::Pic14);
        assert_eq!(count_stores(&m), (0, 1));
        let mut m = module(
            "  %1 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  %2 = or i8 %1, 2\n  store volatile i8 %2, ptr inttoptr (i16 6 to ptr), align 2\n  store volatile i8 %2, ptr inttoptr (i16 7 to ptr), align 2",
        );
        run(&mut m, Core::Pic14);
        assert_eq!(count_stores(&m).0, 0);
    }

    #[test]
    fn status_and_mismatched_addresses_stay() {
        let mut m = module(
            "  %1 = load volatile i8, ptr inttoptr (i16 3 to ptr), align 2\n  %2 = or i8 %1, 2\n  store volatile i8 %2, ptr inttoptr (i16 3 to ptr), align 2\n  %3 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  %4 = or i8 %3, 2\n  store volatile i8 %4, ptr inttoptr (i16 7 to ptr), align 2",
        );
        run(&mut m, Core::Pic14);
        assert_eq!(count_stores(&m), (0, 2));
    }
}
