//! Table PIC18 constant copies in flash instead of materializing them inline (epic-cc#639).
//!
//! Two rewrites, both feeding the const-table memcpy loop epic-cc#504 built:
//! wide loads forwarded from a `const` global become `memcpy`, and runs of
//! constant stores to consecutive addresses become one synthesized `const`
//! global plus a `memcpy`. The table emitter already prints flash-target ref
//! bytes as `db LOW(sym)` / `db HIGH(sym)`, so no backend change is needed.
//! PIC18 only: other cores read const through their own paths, and this pass
//! must not reshape their IR.
//!
//! Store runs convert only with a flash-symbol ref aboard: pure-const runs
//! are #504/#487 territory (clang memcpy and CLRF/SETF coalescing already
//! serve them at better economics than a synthesized table). Zero and 0xFF
//! bytes ride the table at half a word against a 1-word inline CLRF/SETF,
//! so the savings check prices them honestly instead of assuming 2 words
//! per byte.
use std::collections::{HashMap, HashSet};

use device::Core;
use ir::{Gep, GepBase, Global, Inst, MemLen, Memcpy, Module, Param, SrcLoc, Ty, Val};
use iselcore::{resolve_pointers, Base, PtrResolution};

/// Prefilter before the honest savings check below: below this length the
/// 14-word loop plus table can never repay inline, even with no zeros.
const STORE_RUN_MIN_BYTES: usize = 10;
/// A memcpy length rides one byte; longer runs split into several tables.
const MAX_COPY_LEN: usize = 255;

/// Table flash-target constant copies. No-op on every core but PIC18, and
/// in ISR modules: legalize's cross-context rewrite matches stored values
/// and call args, never table refs, so a tabled function address would
/// keep its main-context spelling in ISR code.
pub fn run(m: &mut Module, core: Core) {
    if !matches!(core, Core::Pic18) || m.funcs.iter().any(|f| f.isr) {
        return;
    }
    forward_const_loads(m);
    table_store_runs(m);
}

/// A direct (no dynamic terms) object address inside one function.
#[derive(Clone, PartialEq, Eq)]
enum Root {
    Slot(String),
    Global(String),
}

/// One tableable byte: a literal value or half of a flash-target address.
#[derive(Clone)]
enum RunByte {
    Lit(u8),
    Ref(String),
}

/// The constant bytes a store writes, or `None` when the value is not a
/// compile-time constant this pass tables. RAM-target addresses stay inline:
/// they resolve through the alloc map at emission, a separate question from
/// flash-symbol refs (epic-cc#639 scope).
fn const_bytes(
    val: &Val,
    ty: Ty,
    funcs: &HashSet<String>,
    consts: &HashSet<String>,
) -> Option<Vec<RunByte>> {
    match val {
        Val::Const(c) => {
            let uv = *c as u64;
            Some(
                (0..ty.bytes())
                    .map(|i| RunByte::Lit(((uv >> (8 * i)) & 0xFF) as u8))
                    .collect(),
            )
        }
        Val::Global(g) if ty.bytes() == 2 && (funcs.contains(g) || consts.contains(g)) => {
            Some(vec![RunByte::Ref(g.clone()), RunByte::Ref(g.clone())])
        }
        _ => None,
    }
}

/// Whether `name` is a plain pointer parameter: its slot holds an address
/// rather than the object, so stores through it are indirect (the same
/// distinction isel draws in `emit_ptr_setup`).
fn is_plain_ptr_param(params: &[Param], name: &str) -> bool {
    params
        .iter()
        .any(|p| p.name == name && p.ptr && p.byval.is_none() && !p.sret)
}

/// Resolve a store/load pointer to a direct object address: `(root, offset)`.
/// Indirect slots, computed addresses, and literal pointers have no single
/// tableable destination and return `None`.
fn direct_addr(
    func_name: &str,
    params: &[Param],
    ptr: &str,
    resolved: &PtrResolution,
) -> Option<(Root, u16)> {
    if let Some(g) = ptr.strip_prefix('@') {
        return Some((Root::Global(g.to_string()), 0));
    }
    let r = ptr.strip_prefix('%')?;
    let (base, k, terms) = resolved.get(&iselcore::ssa_key(func_name, r))?;
    if !terms.is_empty() {
        return None;
    }
    match base {
        Base::Slot(s, false) if !is_plain_ptr_param(params, s) => Some((Root::Slot(s.clone()), *k)),
        Base::Global(g) => Some((Root::Global(g.clone()), *k)),
        _ => None,
    }
}

/// A `Val` spelling of a direct address: the root itself at offset 0, else a
/// synthesized `Gep` (mirroring irparse's inlined-GEP shape) for the caller
/// to emit before its user.
fn addr_val(root: &Root, off: u16, func_name: &str, ctr: &mut usize) -> (Val, Option<Inst>) {
    if off == 0 {
        let val = match root {
            Root::Slot(s) => Val::Reg(s.clone()),
            Root::Global(g) => Val::Global(g.clone()),
        };
        return (val, None);
    }
    let base = match root {
        Root::Slot(s) => GepBase::Reg(s.clone()),
        Root::Global(g) => GepBase::Global(g.clone()),
    };
    let dst = format!("__tbl.addr.{func_name}.{ctr}");
    *ctr += 1;
    let gep = Inst::Gep(Gep {
        dst: dst.clone(),
        base,
        k: off,
        terms: Vec::new(),
        loc: None,
    });
    (Val::Reg(dst), Some(gep))
}

/// Register-use counts over one function's blocks, for single-use checks.
fn use_counts(blocks: &[ir::Block]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for b in blocks {
        for i in &b.insts {
            for r in ir::read_vals(i) {
                *counts.entry(r).or_insert(0) += 1;
            }
        }
    }
    counts
}

/// Whether an instruction between a const load and its forwarding store is
/// memory-silent: pure reg ops neither observe nor disturb the half-formed
/// copy. Every memory op closes the window.
fn is_transparent(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Gep(_)
            | Inst::Alloca(_)
            | Inst::Bin(_)
            | Inst::Zext(_)
            | Inst::Sext(_)
            | Inst::Trunc(_)
            | Inst::IntToPtr(_)
            | Inst::Icmp(_)
            | Inst::Select(_)
            | Inst::Phi(_)
            | Inst::Freeze(_)
            | Inst::FloatBin(_)
            | Inst::Fcmp(_)
            | Inst::FloatConv(_)
    )
}

/// A `load` from a `const` global whose only use is a same-width `store`
/// copies the initializer through W at roughly 10 words per byte (a full
/// TBLPTR setup per byte). The same bytes ride one `memcpy`, looped above
/// the copy floor and unrolled below it, either way cheaper.
fn forward_const_loads(m: &mut Module) {
    let consts: HashSet<String> = m
        .globals
        .iter()
        .filter(|g| g.is_const)
        .map(|g| g.name.clone())
        .collect();
    if consts.is_empty() {
        return;
    }
    let resolved = resolve_pointers(m);
    let mut ctr = 0usize;
    for f in &mut m.funcs {
        let uses = use_counts(&f.blocks);
        for b in &mut f.blocks {
            // (load idx, store idx, replacement, setup): scan read-only,
            // applied below in descending index order.
            let mut rewrites: Vec<(usize, usize, Memcpy, Vec<Inst>)> = Vec::new();
            for (li, inst) in b.insts.iter().enumerate() {
                let Inst::Load(l) = inst else { continue };
                if uses.get(&l.dst).copied().unwrap_or(0) != 1 {
                    continue;
                }
                let n = l.ty.bytes() as u16;
                let (src_root, src_off) = match direct_addr(&f.name, &f.params, &l.ptr, &resolved) {
                    Some((Root::Global(g), k)) if consts.contains(&g) => (Root::Global(g), k),
                    _ => continue,
                };
                if src_off.checked_add(n).is_none() {
                    continue;
                }
                let mut si_found: Option<(usize, String, Option<SrcLoc>)> = None;
                for (si, next) in b.insts.iter().enumerate().skip(li + 1) {
                    match next {
                        Inst::Store(s) => {
                            if let Val::Reg(r) = &s.val {
                                if r == &l.dst && s.ty.bytes() as u16 == n {
                                    si_found = Some((si, s.ptr.clone(), s.loc.clone()));
                                }
                            }
                            break;
                        }
                        _ if is_transparent(next) => {}
                        _ => break,
                    }
                }
                let Some((si, dst_ptr, loc)) = si_found else {
                    continue;
                };
                let (dst_root, dst_off) = match direct_addr(&f.name, &f.params, &dst_ptr, &resolved)
                {
                    Some(addr) => addr,
                    None => continue,
                };
                if dst_off.checked_add(n).is_none() {
                    continue;
                }
                let (src_val, src_setup) = addr_val(&src_root, src_off, &f.name, &mut ctr);
                let (dst_val, dst_setup) = addr_val(&dst_root, dst_off, &f.name, &mut ctr);
                let mut setup = Vec::new();
                setup.extend(src_setup);
                setup.extend(dst_setup);
                rewrites.push((
                    li,
                    si,
                    Memcpy {
                        dst: dst_val,
                        src: src_val,
                        len: MemLen::Const(n as u8),
                        loc,
                    },
                    setup,
                ));
            }
            for (li, si, mc, setup) in rewrites.into_iter().rev() {
                b.insts.remove(li);
                let at = si - 1;
                b.insts.remove(at);
                let mut at = at;
                for gep in setup {
                    b.insts.insert(at, gep);
                    at += 1;
                }
                b.insts.insert(at, Inst::Memcpy(mc));
            }
        }
    }
}

/// One member store of an open run: its block position, source location,
/// and byte payload.
struct Member {
    idx: usize,
    loc: Option<SrcLoc>,
    bytes: Vec<RunByte>,
}

/// A run of constant stores to consecutive addresses of one root object.
struct Run {
    root: Root,
    start: u16,
    next: u16,
    members: Vec<Member>,
}

impl Run {
    fn len(&self) -> usize {
        (self.next - self.start) as usize
    }
}

/// Replace runs of constant stores to consecutive addresses with flash
/// table copies. Only the all-constant prefix/suffix converts: a runtime
/// value closes the run, and RAM-target addresses stay inline.
fn table_store_runs(m: &mut Module) {
    let funcs: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    let consts: HashSet<String> = m
        .globals
        .iter()
        .filter(|g| g.is_const)
        .map(|g| g.name.clone())
        .collect();
    let resolved = resolve_pointers(m);
    let mut table: Vec<Global> = Vec::new();
    let mut ctr = 0usize;
    for f in &mut m.funcs {
        for b in &mut f.blocks {
            // Member idxs to remove, and (last member idx, replacement
            // insts): collected during the read-only scan.
            let mut removals: Vec<usize> = Vec::new();
            let mut inserts: Vec<(usize, Vec<Inst>)> = Vec::new();
            let mut open: Option<Run> = None;
            let flush = |open: &mut Option<Run>,
                         removals: &mut Vec<usize>,
                         inserts: &mut Vec<(usize, Vec<Inst>)>,
                         table: &mut Vec<Global>,
                         ctr: &mut usize| {
                let Some(run) = open.take() else { return };
                if run.len() < STORE_RUN_MIN_BYTES {
                    return;
                }
                let n = run.len();
                let mut bytes: Vec<u8> = Vec::with_capacity(n);
                let mut refs: Vec<(usize, String)> = Vec::new();
                let mut cheap = 0usize;
                for member in &run.members {
                    for rb in &member.bytes {
                        match rb {
                            RunByte::Lit(v) if *v == 0 || *v == 0xFF => {
                                cheap += 1;
                                bytes.push(*v);
                            }
                            RunByte::Lit(v) => bytes.push(*v),
                            RunByte::Ref(sym) => {
                                refs.push((bytes.len(), sym.clone()));
                                bytes.push(0);
                            }
                        }
                    }
                }
                // Pure-const runs are #504/#487 territory (clang memcpy and
                // CLRF/SETF coalescing already serve them); this pass tables
                // only runs carrying flash-symbol refs, the #639 scope.
                // Inline costs 2 words per byte but 1 per zero (CLRF) or
                // 0xFF (SETF), against a 14-word loop plus half a word per
                // table byte: convert only strictly profitable runs.
                if refs.is_empty() {
                    return;
                }
                if 2 * n - cheap < 14 + (n + 1) / 2 + 1 {
                    return;
                }
                let name = format!("__tbl.init.{}.{ctr}", f.name);
                *ctr += 1;
                let global = Global {
                    name: name.clone(),
                    ty: Ty::I8,
                    is_const: true,
                    size: n as u16,
                    bytes,
                    refs,
                    addr: None,
                };
                let (dst_val, setup) = addr_val(&run.root, run.start, &f.name, ctr);
                let last = run.members.last().expect("run has members").idx;
                let loc = run.members.last().and_then(|member| member.loc.clone());
                let mut insts = Vec::new();
                insts.extend(setup);
                insts.push(Inst::Memcpy(Memcpy {
                    dst: dst_val,
                    src: Val::Global(name),
                    len: MemLen::Const(n as u8),
                    loc,
                }));
                for member in &run.members {
                    removals.push(member.idx);
                }
                table.push(global);
                inserts.push((last, insts));
            };
            for (idx, inst) in b.insts.iter().enumerate() {
                match inst {
                    Inst::Store(s) => {
                        let addr = direct_addr(&f.name, &f.params, &s.ptr, &resolved);
                        let payload = const_bytes(&s.val, s.ty, &funcs, &consts);
                        match (addr, payload) {
                            (Some((root, off)), Some(bytes)) => {
                                let len = bytes.len() as u16;
                                let adjacent = match &open {
                                    Some(run) => {
                                        run.root == root
                                            && run.next == off
                                            && run.len() + bytes.len() <= MAX_COPY_LEN
                                    }
                                    None => false,
                                };
                                if off.checked_add(len).is_none() {
                                    flush(
                                        &mut open,
                                        &mut removals,
                                        &mut inserts,
                                        &mut table,
                                        &mut ctr,
                                    );
                                } else if adjacent {
                                    let run = open.as_mut().expect("open run");
                                    run.next += len;
                                    run.members.push(Member {
                                        idx,
                                        loc: s.loc.clone(),
                                        bytes,
                                    });
                                } else {
                                    flush(
                                        &mut open,
                                        &mut removals,
                                        &mut inserts,
                                        &mut table,
                                        &mut ctr,
                                    );
                                    open = Some(Run {
                                        root,
                                        start: off,
                                        next: off + len,
                                        members: vec![Member {
                                            idx,
                                            loc: s.loc.clone(),
                                            bytes,
                                        }],
                                    });
                                }
                            }
                            _ => {
                                flush(&mut open, &mut removals, &mut inserts, &mut table, &mut ctr)
                            }
                        }
                    }
                    _ if is_transparent(inst) => {}
                    _ => flush(&mut open, &mut removals, &mut inserts, &mut table, &mut ctr),
                }
            }
            flush(&mut open, &mut removals, &mut inserts, &mut table, &mut ctr);
            removals.sort_unstable();
            for idx in removals.iter().rev() {
                b.insts.remove(*idx);
            }
            inserts.sort_by_key(|(last, _)| *last);
            let mut shift = 0usize;
            for (last, insts) in inserts {
                // `last` is a pre-removal index: subtract the members
                // removed before it, then add the insts already inserted.
                let removed_before = removals.iter().filter(|r| **r < last).count();
                let at = last - removed_before + shift;
                let count = insts.len();
                for (j, inst) in insts.into_iter().enumerate() {
                    b.insts.insert(at + j, inst);
                }
                shift += count;
            }
        }
    }
    m.globals.extend(table);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::{Alloca, Block, Func, Gep, Load, Store};

    fn empty_module() -> Module {
        Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        }
    }

    fn main_func(insts: Vec<Inst>) -> Func {
        Func {
            name: "main".to_string(),
            ret: None,
            params: Vec::new(),
            blocks: vec![Block {
                label: "entry".to_string(),
                insts,
            }],
            isr: false,
            irq_priority: 0,
            naked: false,
            variadic: false,
        }
    }

    /// ISR modules keep pre-pass IR: legalize's cross-context rewrite
    /// matches stored values and call args, never table refs, so a tabled
    /// function address would keep its main-context spelling in ISR code.
    #[test]
    fn isr_modules_keep_their_ir() {
        let mut m = empty_module();
        m.funcs.push(Func {
            name: "f".to_string(),
            ret: None,
            params: Vec::new(),
            blocks: vec![Block {
                label: "entry".to_string(),
                insts: vec![Inst::Ret(None, None)],
            }],
            isr: false,
            irq_priority: 0,
            naked: false,
            variadic: false,
        });
        let mut insts = vec![Inst::Alloca(Alloca {
            dst: "a".to_string(),
            size: 12,
            loc: None,
        })];
        insts.push(Inst::Store(Store {
            ty: Ty::I16,
            val: Val::Global("f".to_string()),
            ptr: "%a".to_string(),
            loc: None,
        }));
        for (g, k) in [("g2", 2u16), ("g3", 6u16), ("g4", 10u16)] {
            insts.push(Inst::Gep(Gep {
                dst: g.to_string(),
                base: GepBase::Reg("a".to_string()),
                k,
                terms: Vec::new(),
                loc: None,
            }));
            let ty = if k == 10 { Ty::I16 } else { Ty::I32 };
            insts.push(Inst::Store(Store {
                ty,
                val: Val::Const(0x01020304),
                ptr: format!("%{g}"),
                loc: None,
            }));
        }
        let mut isr = main_func(insts);
        isr.name = "isr".to_string();
        isr.isr = true;
        m.funcs.push(isr);
        run(&mut m, Core::Pic18);
        assert!(m.globals.is_empty(), "no table may form in an ISR module");
        assert_eq!(m.funcs[1].blocks[0].insts.len(), 8, "stores stay inline");
    }

    #[test]
    fn non_pic18_cores_keep_their_ir() {
        let mut m = empty_module();
        m.funcs.push(main_func(Vec::new()));
        run(&mut m, Core::Pic14);
        assert!(m.globals.is_empty());
    }

    #[test]
    fn const_load_forwarded_to_a_store_becomes_memcpy() {
        let mut m = empty_module();
        m.globals.push(Global {
            name: "c".to_string(),
            ty: Ty::I8,
            is_const: true,
            size: 8,
            bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
            refs: Vec::new(),
            addr: None,
        });
        m.funcs.push(main_func(vec![
            Inst::Alloca(Alloca {
                dst: "a".to_string(),
                size: 8,
                loc: None,
            }),
            Inst::Load(Load {
                dst: "v".to_string(),
                ty: Ty::I64,
                ptr: "@c".to_string(),
                ptr_ty: false,
                loc: None,
            }),
            Inst::Store(Store {
                ty: Ty::I64,
                val: Val::Reg("v".to_string()),
                ptr: "%a".to_string(),
                loc: None,
            }),
        ]));
        run(&mut m, Core::Pic18);
        let insts = &m.funcs[0].blocks[0].insts;
        assert_eq!(insts.len(), 2, "load+store become one memcpy: {insts:?}");
        match &insts[1] {
            Inst::Memcpy(mc) => {
                assert_eq!(mc.src, Val::Global("c".to_string()));
                assert_eq!(mc.dst, Val::Reg("a".to_string()));
                assert_eq!(mc.len, MemLen::Const(8));
            }
            other => panic!("expected memcpy, got {other:?}"),
        }
    }

    #[test]
    fn multi_use_const_load_is_left_alone() {
        let mut m = empty_module();
        m.globals.push(Global {
            name: "c".to_string(),
            ty: Ty::I8,
            is_const: true,
            size: 8,
            bytes: vec![0; 8],
            refs: Vec::new(),
            addr: None,
        });
        m.funcs.push(main_func(vec![
            Inst::Alloca(Alloca {
                dst: "a".to_string(),
                size: 8,
                loc: None,
            }),
            Inst::Alloca(Alloca {
                dst: "b".to_string(),
                size: 8,
                loc: None,
            }),
            Inst::Load(Load {
                dst: "v".to_string(),
                ty: Ty::I64,
                ptr: "@c".to_string(),
                ptr_ty: false,
                loc: None,
            }),
            Inst::Store(Store {
                ty: Ty::I64,
                val: Val::Reg("v".to_string()),
                ptr: "%a".to_string(),
                loc: None,
            }),
            Inst::Store(Store {
                ty: Ty::I64,
                val: Val::Reg("v".to_string()),
                ptr: "%b".to_string(),
                loc: None,
            }),
        ]));
        run(&mut m, Core::Pic18);
        assert_eq!(m.funcs[0].blocks[0].insts.len(), 5);
    }

    /// Twelve consecutive constant bytes (a function address plus ten
    /// literal bytes through GEPs) become one table plus one memcpy.
    /// `f` must exist as a function for its address to table.
    #[test]
    fn twelve_constant_bytes_with_a_flash_ref_become_one_table() {
        let mut m = empty_module();
        m.funcs.push(Func {
            name: "f".to_string(),
            ret: None,
            params: Vec::new(),
            blocks: vec![Block {
                label: "entry".to_string(),
                insts: vec![Inst::Ret(None, None)],
            }],
            isr: false,
            irq_priority: 0,
            naked: false,
            variadic: false,
        });
        let mut insts = vec![Inst::Alloca(Alloca {
            dst: "a".to_string(),
            size: 12,
            loc: None,
        })];
        insts.push(Inst::Store(Store {
            ty: Ty::I16,
            val: Val::Global("f".to_string()),
            ptr: "%a".to_string(),
            loc: None,
        }));
        for (g, k, ty, val) in [
            ("g2", 2u16, Ty::I32, Val::Const(0x01020304)),
            ("g3", 6u16, Ty::I32, Val::Const(0x05060708)),
            ("g4", 10u16, Ty::I16, Val::Const(0x090A)),
        ] {
            insts.push(Inst::Gep(Gep {
                dst: g.to_string(),
                base: GepBase::Reg("a".to_string()),
                k,
                terms: Vec::new(),
                loc: None,
            }));
            insts.push(Inst::Store(Store {
                ty,
                val,
                ptr: format!("%{g}"),
                loc: None,
            }));
        }
        m.funcs.push(main_func(insts));
        run(&mut m, Core::Pic18);
        let tbl = m
            .globals
            .iter()
            .find(|g| g.is_const && g.name.starts_with("__tbl.init.main."));
        let tbl = tbl.expect("a table must be synthesized");
        assert_eq!(tbl.bytes.len(), 12);
        assert_eq!(tbl.refs, vec![(0, "f".to_string()), (1, "f".to_string())]);
        assert_eq!(tbl.bytes[2], 0x04, "little-endian literal layout");
        let insts = &m.funcs[1].blocks[0].insts;
        let copies: Vec<_> = insts
            .iter()
            .filter(|i| matches!(i, Inst::Memcpy(_)))
            .collect();
        assert_eq!(
            copies.len(),
            1,
            "one memcpy replaces four stores: {insts:?}"
        );
        match copies[0] {
            Inst::Memcpy(mc) => {
                assert_eq!(mc.dst, Val::Reg("a".to_string()));
                assert_eq!(mc.len, MemLen::Const(12));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn short_runs_and_runtime_values_stay_inline() {
        let mut m = empty_module();
        m.funcs.push(main_func(vec![
            Inst::Alloca(Alloca {
                dst: "a".to_string(),
                size: 4,
                loc: None,
            }),
            Inst::Store(Store {
                ty: Ty::I32,
                val: Val::Const(1),
                ptr: "%a".to_string(),
                loc: None,
            }),
            Inst::Store(Store {
                ty: Ty::I32,
                val: Val::Reg("x".to_string()),
                ptr: "%a".to_string(),
                loc: None,
            }),
        ]));
        run(&mut m, Core::Pic18);
        assert!(
            m.globals.is_empty(),
            "no table below the floor or for runtime values"
        );
        assert_eq!(m.funcs[0].blocks[0].insts.len(), 3);
    }

    /// Twelve pure-const bytes stay inline: without a flash-symbol ref the
    /// run is #504/#487 territory, never this pass's (epic-cc#639 scope).
    #[test]
    fn pure_const_run_stays_inline_despite_size() {
        let mut m = empty_module();
        let mut insts = vec![Inst::Alloca(Alloca {
            dst: "a".to_string(),
            size: 12,
            loc: None,
        })];
        for (g, k) in [("g1", 0u16), ("g2", 4u16), ("g3", 8u16)] {
            insts.push(Inst::Gep(Gep {
                dst: g.to_string(),
                base: GepBase::Reg("a".to_string()),
                k,
                terms: Vec::new(),
                loc: None,
            }));
            insts.push(Inst::Store(Store {
                ty: Ty::I32,
                val: Val::Const(0x01020304),
                ptr: format!("%{g}"),
                loc: None,
            }));
        }
        m.funcs.push(main_func(insts));
        run(&mut m, Core::Pic18);
        assert!(
            m.globals.is_empty(),
            "a run with no flash ref must not table"
        );
    }

    /// Two ref bytes drowned in ten zeros stay inline: zeros cost 1 word
    /// inline against half a table word, so the honest math loses here.
    #[test]
    fn zero_heavy_ref_run_stays_inline() {
        let mut m = empty_module();
        m.funcs.push(Func {
            name: "f".to_string(),
            ret: None,
            params: Vec::new(),
            blocks: vec![Block {
                label: "entry".to_string(),
                insts: vec![Inst::Ret(None, None)],
            }],
            isr: false,
            irq_priority: 0,
            naked: false,
            variadic: false,
        });
        let mut insts = vec![Inst::Alloca(Alloca {
            dst: "a".to_string(),
            size: 12,
            loc: None,
        })];
        insts.push(Inst::Store(Store {
            ty: Ty::I16,
            val: Val::Global("f".to_string()),
            ptr: "%a".to_string(),
            loc: None,
        }));
        for (g, k, ty) in [
            ("g2", 2u16, Ty::I32),
            ("g3", 6u16, Ty::I32),
            ("g4", 10u16, Ty::I16),
        ] {
            insts.push(Inst::Gep(Gep {
                dst: g.to_string(),
                base: GepBase::Reg("a".to_string()),
                k,
                terms: Vec::new(),
                loc: None,
            }));
            insts.push(Inst::Store(Store {
                ty,
                val: Val::Const(0),
                ptr: format!("%{g}"),
                loc: None,
            }));
        }
        m.funcs.push(main_func(insts));
        run(&mut m, Core::Pic18);
        assert!(m.globals.is_empty(), "a zero-heavy run must not table");
    }

    /// Two ref bytes drowned in ten 0xFF bytes stay inline: 0xFF costs 1
    /// word inline (SETF), so the honest math loses here just like zeros.
    #[test]
    fn ff_heavy_ref_run_stays_inline() {
        let mut m = empty_module();
        m.funcs.push(Func {
            name: "f".to_string(),
            ret: None,
            params: Vec::new(),
            blocks: vec![Block {
                label: "entry".to_string(),
                insts: vec![Inst::Ret(None, None)],
            }],
            isr: false,
            irq_priority: 0,
            naked: false,
            variadic: false,
        });
        let mut insts = vec![Inst::Alloca(Alloca {
            dst: "a".to_string(),
            size: 12,
            loc: None,
        })];
        insts.push(Inst::Store(Store {
            ty: Ty::I16,
            val: Val::Global("f".to_string()),
            ptr: "%a".to_string(),
            loc: None,
        }));
        for (g, k, ty, val) in [
            ("g2", 2u16, Ty::I32, Val::Const(-1)),
            ("g3", 6u16, Ty::I32, Val::Const(-1)),
            ("g4", 10u16, Ty::I16, Val::Const(-1)),
        ] {
            insts.push(Inst::Gep(Gep {
                dst: g.to_string(),
                base: GepBase::Reg("a".to_string()),
                k,
                terms: Vec::new(),
                loc: None,
            }));
            insts.push(Inst::Store(Store {
                ty,
                val,
                ptr: format!("%{g}"),
                loc: None,
            }));
        }
        m.funcs.push(main_func(insts));
        run(&mut m, Core::Pic18);
        assert!(m.globals.is_empty(), "a 0xFF-heavy run must not table");
    }
}
