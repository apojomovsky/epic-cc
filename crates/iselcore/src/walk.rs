//! Walk-loop temp roles for epic-cc#1370, shared by `alloc`, `isel`, and
//! the driver size report. A counted single-block loop the walk lowering
//! fuses (counter to `DECFSZ`, latch compare dropped, index chains rebuilt
//! from the down-counter) leaves temps no emission reads: the increment,
//! the latch compare, and dead chain/trunc/link regs take no home,
//! single-use loads feeding an accumulate add stay in W, and the add
//! shares its carried phi's home.
//!
//! One analysis serves all three so a dropped slot is never read: the
//! match below replicates `isel`'s single-block acceptance exactly, minus
//! frame addresses. Every address-dependent check answers conservatively
//! (slot bases decline, unresolvable pointers decline, pool-risk regs
//! decline), so an accepted loop is a subset of what the backend fuses.
//! Latch-out loops keep their homes: only the single-block shape matches.
//!
//! The window proof reads the globals map the caller passes. `alloc`
//! passes its bounding map and `isel` its final map; the two agree on
//! globals (the exactness rule behind `allocate_with_pool`'s two passes),
//! so both reach the same verdict. Synthetic functions outside the module
//! (the pool-log variant) never match.

use std::collections::{HashMap, HashSet};

use ir::{BinOp, Inst, Module, Val};

use crate::{
    const_fan_in, ssa_key, Base, FlashProvenance, LoadFold, PtrResolution, ValueFolds,
    LOG_POOL_CALLEE,
};

/// Inputs to the walk match. `unplaced` is the caller's own folds drop
/// set (`alloc` passes its gated folds, `isel` its `w_folds` set); on
/// PIC14 both derive from the same mapped globals, so they agree.
/// `globals` maps global names to addresses; `ram_banks` and `common_ram`
/// are the device windows copied as plain data, keeping `device` out of
/// this crate.
pub struct WalkCtx<'a> {
    pub resolved: &'a PtrResolution,
    pub folds: &'a ValueFolds,
    pub unplaced: &'a HashSet<String>,
    pub prov: &'a FlashProvenance,
    pub globals: &'a HashMap<String, u16>,
    pub ram_banks: &'a [(u16, u16)],
    pub common_ram: Option<(u16, u16)>,
    pub module: &'a Module,
    /// True on classic PIC14 only. Other cores keep every home, so their
    /// backends stay byte-identical while this work lands.
    pub pic14: bool,
}

/// One accepted single-block walk loop: the counter and trip count plus
/// the temp roles. `dead` names regs taking no home, `wloads` the
/// single-use loads staying in W, `acc` the `(add, phi)` accumulate pairs
/// sharing one home, `latch` the fused latch compare for the scratch
/// model.
pub struct WalkMatch {
    pub header: String,
    pub pre: String,
    pub exit: String,
    pub cntreg: String,
    pub increg: String,
    pub latch: String,
    pub k: u16,
    pub up: bool,
    pub dead: HashSet<String>,
    pub wloads: HashSet<String>,
    pub acc: Vec<(String, String)>,
}

/// Match every single-block walk loop in `f`. Loops the match declines
/// keep today's homes.
pub fn match_walks(m: &Module, f: &ir::Func, ctx: &WalkCtx) -> Vec<WalkMatch> {
    if !ctx.pic14 {
        return Vec::new();
    }
    if !m.funcs.iter().any(|x| x.name == f.name) {
        return Vec::new();
    }
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &f.blocks {
        let targets: Vec<&str> = match b.insts.last() {
            Some(Inst::Br(br)) => vec![br.target.as_str()],
            Some(Inst::BrCond(bc)) => vec![bc.t.as_str(), bc.f.as_str()],
            _ => vec![],
        };
        for t in targets {
            preds.entry(t).or_default().push(b.label.as_str());
        }
    }
    let mut uses: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for b in &f.blocks {
        for (ii, inst) in b.insts.iter().enumerate() {
            for r in ir::read_vals(inst) {
                if r.is_empty() || r.starts_with('@') {
                    continue;
                }
                uses.entry(r).or_default().push((b.label.clone(), ii));
            }
        }
    }
    let mut out = Vec::new();
    for b in &f.blocks {
        if let Some(wm) = match_one(f, b, &preds, &uses, ctx) {
            out.push(wm);
        }
    }
    out
}

/// `{func}::{reg}` slots the pool-log variant rewrites to pool indices
/// (epic-cc#817): the fmt param, closed over phi/gep/select derivations.
/// Mirrors `isel`'s closure so the match declines what the variant must.
fn pool_slot_closure(f: &ir::Func) -> HashSet<String> {
    let key = |n: &str| ssa_key(&f.name, n);
    let mut set = HashSet::new();
    if let Some(p) = f.params.iter().find(|p| p.ptr) {
        set.insert(key(&p.name));
    }
    let mut progressed = true;
    while progressed {
        progressed = false;
        for b in &f.blocks {
            for inst in &b.insts {
                let dst = match inst {
                    Inst::Phi(p) if p.ptr => {
                        let any = p
                            .incoming
                            .iter()
                            .any(|(v, _)| matches!(v, Val::Reg(r) if set.contains(&key(r))));
                        any.then(|| p.dst.clone())
                    }
                    Inst::Gep(g) => match &g.base {
                        ir::GepBase::Reg(b) if set.contains(&key(b)) => Some(g.dst.clone()),
                        _ => None,
                    },
                    Inst::Select(s) if s.ptr => {
                        let arm = |v: &Val| matches!(v, Val::Reg(r) if set.contains(&key(r)));
                        (arm(&s.a) || arm(&s.b)).then(|| s.dst.clone())
                    }
                    _ => None,
                };
                if let Some(d) = dst {
                    if set.insert(key(&d)) {
                        progressed = true;
                    }
                }
            }
        }
    }
    set
}

/// Whether a walk header block names any pool-index slot, as a use or a
/// def. Either way the variant's divergence owns the loop, so the match
/// stays out and every home survives for the variant's normal emission.
fn block_touches_pool_slots(func: &str, b: &ir::Block, slots: &HashSet<String>) -> bool {
    let key = |n: &str| ssa_key(func, n);
    for inst in &b.insts {
        for r in ir::read_vals(inst) {
            if !r.is_empty() && !r.starts_with('@') && slots.contains(&key(&r)) {
                return true;
            }
        }
        let dst: Option<&String> = match inst {
            Inst::Phi(p) => Some(&p.dst),
            Inst::Bin(q) => Some(&q.dst),
            Inst::Gep(g) => Some(&g.dst),
            Inst::Load(l) => Some(&l.dst),
            Inst::Trunc(t) => Some(&t.dst),
            Inst::Zext(z) => Some(&z.dst),
            Inst::Sext(s) => Some(&s.dst),
            Inst::Icmp(c) => Some(&c.dst),
            Inst::Select(s) => Some(&s.dst),
            Inst::Call(c) => c.dst.as_ref(),
            _ => None,
        };
        if dst.is_some_and(|d| slots.contains(&key(d))) {
            return true;
        }
    }
    false
}

fn match_one(
    f: &ir::Func,
    b: &ir::Block,
    preds: &HashMap<&str, Vec<&str>>,
    uses: &HashMap<String, Vec<(String, usize)>>,
    ctx: &WalkCtx,
) -> Option<WalkMatch> {
    let last = b.insts.len().checked_sub(1)?;
    let bc = match &b.insts[last] {
        Inst::BrCond(bc) => bc,
        _ => return None,
    };
    if bc.t == bc.f {
        return None;
    }
    let exit = if bc.t == b.label {
        bc.f.clone()
    } else if bc.f == b.label {
        bc.t.clone()
    } else {
        return None;
    };
    let Val::Reg(cond) = &bc.cond else {
        return None;
    };
    if last == 0 {
        return None;
    }
    // The latch compare sits immediately before the branch, so its feed
    // cone is inside this block: dropping it strands no cross-block reader.
    let latch_ii = last - 1;
    let latch = match &b.insts[latch_ii] {
        Inst::Icmp(c) if c.dst == *cond && c.pred == "eq" => c.clone(),
        _ => return None,
    };
    // A materialized second use would survive the fused branch; the walk
    // drops the compare, so it must be the branch's alone.
    if uses.get(&latch.dst).map(|v| v.len()) != Some(1) {
        return None;
    }
    let (increg, cmp_c) = match (&latch.a, &latch.b) {
        (Val::Reg(r), Val::Const(c)) | (Val::Const(c), Val::Reg(r)) => (r.clone(), *c),
        _ => return None,
    };
    let (inc_ii, incb) = b.insts.iter().enumerate().find_map(|(ii, i)| match i {
        Inst::Bin(q) if q.dst == increg => Some((ii, q.clone())),
        _ => None,
    })?;
    let (cntreg, up) = walk_inc_counter(&incb)?;
    if cntreg == increg {
        return None;
    }
    // The counter keeps its home (the down-counter slot), so a folded
    // counter refuses: the backend declines the walk without one.
    if ctx.unplaced.contains(&cntreg) {
        return None;
    }
    let (phi_ii, phi) = b.insts.iter().enumerate().find_map(|(ii, i)| match i {
        Inst::Phi(p) if p.dst == cntreg => Some((ii, p.clone())),
        _ => None,
    })?;
    let (pre, k) = walk_phi_k(&phi, &increg, up, cmp_c, &b.label)?;
    if pre == b.label || pre == exit {
        return None;
    }
    let pre_block = f.blocks.iter().find(|x| x.label == pre)?;
    if !matches!(&pre_block.insts.last(), Some(Inst::Br(br)) if br.target == b.label) {
        return None;
    }
    // Pool-log variant guard (epic-cc#1409): the variant rewrites the fmt
    // param's phi/gep/select derivations to pool indices and declines any
    // walk touching them, while this match only declines const-derived
    // pool risk. A loop the variant must decline keeps every home here,
    // so the variant's normal emission never names a dropped slot.
    if f.name == LOG_POOL_CALLEE {
        let slots = pool_slot_closure(f);
        if block_touches_pool_slots(&f.name, b, &slots) {
            return None;
        }
    }
    match preds.get(b.label.as_str()) {
        Some(ps)
            if ps.len() == 2 && ps.contains(&pre.as_str()) && ps.contains(&b.label.as_str()) => {}
        _ => return None,
    }
    let body = match_body(
        f, b, uses, ctx, &cntreg, &increg, up, k, phi_ii, inc_ii, latch_ii,
    )?;
    let mut dead: HashSet<String> = HashSet::from([increg.clone(), latch.dst.clone()]);
    dead.extend(body.vadds.iter().cloned());
    dead.extend(body.anddefs.iter().cloned());
    dead.extend(body.truncs.iter().cloned());
    let (wloads, acc) = accumulate_roles(b, uses, ctx, &body);
    Some(WalkMatch {
        header: b.label.clone(),
        pre,
        exit,
        cntreg,
        increg,
        latch: latch.dst,
        k,
        up,
        dead,
        wloads,
        acc,
    })
}

/// Shared counter/increment match for both counting directions. Anything
/// else keeps counting the old way.
fn walk_inc_counter(incb: &ir::Bin) -> Option<(String, bool)> {
    match (&incb.op, &incb.a, &incb.b) {
        (BinOp::Add, Val::Reg(q), Val::Const(1)) => Some((q.clone(), true)),
        (BinOp::Add, Val::Const(1), Val::Reg(q)) => Some((q.clone(), true)),
        (BinOp::Add, Val::Reg(q), Val::Const(-1)) => Some((q.clone(), false)),
        (BinOp::Sub, Val::Reg(q), Val::Const(1)) => Some((q.clone(), false)),
        _ => None,
    }
}

/// Counter phi check: a byte counter with one literal entry arm and one
/// backedge arm over the counter or its increment. Zero trips would run
/// the body 256 times under DECFSZ, so K = 0 stays out.
fn walk_phi_k(
    phi: &ir::Phi,
    increg: &str,
    up: bool,
    cmp_c: i64,
    back: &str,
) -> Option<(String, u16)> {
    if !(phi.ty.bytes() == 1 || phi.ty.bytes() == 2) || phi.ptr {
        return None;
    }
    if phi.incoming.len() != 2 {
        return None;
    }
    let mut pre: Option<String> = None;
    let mut init_c: Option<i64> = None;
    let mut back_ok = false;
    for (v, e) in &phi.incoming {
        if e == back {
            if !matches!(v, Val::Reg(q) if q == &phi.dst || q.as_str() == increg) {
                return None;
            }
            back_ok = true;
        } else {
            if pre.is_some() {
                return None;
            }
            let Val::Const(c) = v else {
                return None;
            };
            init_c = Some(*c);
            pre = Some(e.clone());
        }
    }
    if !back_ok {
        return None;
    }
    // Up loops start at zero and exit when the increment reaches the
    // bound; down loops exit at zero, so the start is the trip count.
    let k = if up {
        if init_c != Some(0) {
            return None;
        }
        cmp_c
    } else {
        if cmp_c != 0 {
            return None;
        }
        init_c?
    };
    if !(1..=255).contains(&k) {
        return None;
    }
    Some((pre?, k as u16))
}

fn walk_mentions(v: &Val, cntreg: &str, increg: &str) -> bool {
    matches!(v, Val::Reg(r) if r == cntreg || r == increg)
}

/// Accumulate pairs (cut 2) with their W loads (cut 3): a region load
/// whose only use is the next instruction, a one-byte add of the load
/// and a carried phi of this loop. The add shares the phi's home and the
/// load stays in W; the backend emits both as one `ADDWF` unit. A folded
/// add stays out: its store destination is not the phi's home.
fn accumulate_roles(
    b: &ir::Block,
    uses: &HashMap<String, Vec<(String, usize)>>,
    ctx: &WalkCtx,
    body: &BodyRoles,
) -> (HashSet<String>, Vec<(String, String)>) {
    let mut wloads = HashSet::new();
    let mut acc = Vec::new();
    for (load_dst, (lb, li)) in &body.loads {
        if uses.get(load_dst).map(|v| v.as_slice()) != Some(&[(lb.clone(), li + 1)]) {
            continue;
        }
        let Some(Inst::Bin(q)) = b.insts.get(li + 1) else {
            continue;
        };
        if q.op != BinOp::Add || q.ty.bytes() != 1 {
            continue;
        }
        if ctx.unplaced.contains(&q.dst) {
            continue;
        }
        let phi = match (&q.a, &q.b) {
            (Val::Reg(r), Val::Reg(p)) | (Val::Reg(p), Val::Reg(r)) if r == load_dst => {
                match b.insts.iter().find_map(|i| match i {
                    Inst::Phi(p2) if p2.dst == *p => Some(p2),
                    _ => None,
                }) {
                    Some(p2) if !p2.ptr && p2.ty.bytes() == 1 => p2,
                    _ => continue,
                }
            }
            _ => continue,
        };
        // The phi carries the add around the backedge: one incoming is
        // the add on the self edge, the other the entry seed.
        if phi.dst == body.cntreg {
            continue;
        }
        let mut back_ok = false;
        for (v, e) in &phi.incoming {
            if e == &b.label && *v == Val::Reg(q.dst.clone()) {
                back_ok = true;
            }
        }
        if !back_ok {
            continue;
        }
        // Intra-loop readers would see the phi's current value, not the
        // add's next one; outside readers run past the exit copy, which
        // holds the final add value. The phi's own incoming is the
        // backedge copy the latch already runs.
        let phi_pos = b
            .insts
            .iter()
            .position(|i| matches!(i, Inst::Phi(p2) if p2.dst == phi.dst));
        let phi_site = (b.label.clone(), phi_pos.unwrap_or(usize::MAX));
        let ok = match uses.get(&q.dst) {
            None => false,
            Some(u) => u.iter().all(|s| *s == phi_site || s.0 != b.label),
        };
        if !ok {
            continue;
        }
        wloads.insert(load_dst.clone());
        acc.push((q.dst.clone(), phi.dst.clone()));
    }
    (wloads, acc)
}
struct BodyRoles {
    cntreg: String,
    vadds: HashSet<String>,
    anddefs: HashSet<String>,
    truncs: HashSet<String>,
    loads: Vec<(String, (String, usize))>,
}

struct AndDef {
    mask: u8,
    trunc: Option<(String, usize)>,
    site: (String, usize),
}

enum WalkAdd {
    No,
    PostInc,
    Direct(Val),
    ViaTrunc((String, usize), Val),
}

/// Classify a byte-wide `Add` against the counter: either side may hold
/// the pre-increment counter (direct or through a trunc of it). A masked
/// index link as a side is the masked classifier's own, a post-increment
/// read is unservable, anything else is ordinary ALU.
fn walk_value_add(
    op: &BinOp,
    a: &Val,
    b: &Val,
    width: u8,
    cntreg: &str,
    increg: &str,
    ctr_truncs: &HashMap<String, (String, usize)>,
    anddefs: &HashMap<String, AndDef>,
) -> WalkAdd {
    if *op != BinOp::Add || width != 1 {
        return WalkAdd::No;
    }
    if matches!(a, Val::Reg(x) if x == cntreg) || matches!(b, Val::Reg(x) if x == cntreg) {
        let base = if matches!(a, Val::Reg(x) if x == cntreg) {
            b.clone()
        } else {
            a.clone()
        };
        if matches!(&base, Val::Reg(x) if x == cntreg || anddefs.contains_key(x)) {
            return WalkAdd::No;
        }
        return WalkAdd::Direct(base);
    }
    for (side, other) in [(a, b), (b, a)] {
        if let Val::Reg(x) = side {
            if let Some(tii) = ctr_truncs.get(x) {
                // The other side must be a real base: a second counter
                // trunc has no slot to rebuild from, and a masked link is
                // the masked classifier's own.
                if matches!(other, Val::Reg(y) if ctr_truncs.contains_key(y) || anddefs.contains_key(y))
                {
                    return WalkAdd::No;
                }
                return WalkAdd::ViaTrunc(tii.clone(), other.clone());
            }
        }
    }
    if matches!(a, Val::Reg(x) if x == increg) || matches!(b, Val::Reg(x) if x == increg) {
        return WalkAdd::PostInc;
    }
    WalkAdd::No
}

/// Classify a byte-wide `Add` with a masked index link as a side:
/// `add base (and (counter | trunc counter) mask)`. The link arm calls
/// this for every use and the add arm for the add itself, so both agree.
fn walk_masked_vadd(
    q: &ir::Bin,
    cntreg: &str,
    ctr_truncs: &HashMap<String, (String, usize)>,
    anddefs: &HashMap<String, AndDef>,
    up: bool,
    site: (String, usize),
) -> Option<((String, usize), Option<(String, usize)>, Val, (u8, String))> {
    if q.op != BinOp::Add || q.ty.bytes() != 1 || !up {
        return None;
    }
    for (side, other) in [(&q.a, &q.b), (&q.b, &q.a)] {
        if let Val::Reg(x) = side {
            if let Some(def) = anddefs.get(x) {
                if matches!(other, Val::Reg(y) if y == cntreg
                    || ctr_truncs.contains_key(y)
                    || anddefs.contains_key(y))
                {
                    return None;
                }
                return Some((
                    site,
                    def.trunc.clone(),
                    other.clone(),
                    (def.mask, x.clone()),
                ));
            }
        }
    }
    None
}

/// Whether a non-counter `Bin` can share a walk body: pure ALU the normal
/// arm lowers without touching `FSR`. Anything that might call a helper
/// or move a pointer stays out.
fn walk_plain_bin(q: &ir::Bin) -> bool {
    match q.op {
        BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Or | BinOp::Xor => true,
        BinOp::Shl | BinOp::LShr | BinOp::AShr => {
            matches!(q.a, Val::Const(_)) || matches!(q.b, Val::Const(_))
        }
        _ => false,
    }
}

/// A const global without a RAM copy in the fan-in: the backend's flash
/// and pool refusals fire on such values, so the match declines where it
/// cannot see the backend's own sets. Values over mapped consts lower
/// through RAM and stay eligible.
fn pool_risk(ctx: &WalkCtx, func: &str, v: &Val) -> bool {
    const_fan_in(ctx.module, ctx.resolved, func, v)
        .iter()
        .any(|g| {
            ctx.module
                .globals
                .iter()
                .any(|gg| &gg.name == g && gg.is_const && !ctx.globals.contains_key(g))
        })
}

fn global_is_const(m: &Module, name: &str) -> bool {
    m.globals.iter().any(|g| g.name == name && g.is_const)
}

/// Validate one walk memory op's pointer without frame addresses: a
/// counter-indexed access over one `(base, k)` group with a window-safe
/// span. Slot bases decline (their homes are layout-dependent); so do
/// unresolvable pointers and flash-risk regs.
#[allow(clippy::too_many_arguments)]
fn walk_memop_ok(
    f: &ir::Func,
    cntreg: &str,
    ptr: &str,
    k: u16,
    ctx: &WalkCtx,
    gep_users: &mut HashMap<String, Vec<(String, usize)>>,
    term_sites: &mut Vec<(String, usize)>,
    site: (String, usize),
) -> Option<(bool, u8)> {
    let preg = ptr.strip_prefix('%')?;
    let key = ssa_key(&f.name, preg);
    if ctx.prov.flash.contains(&key) || ctx.prov.mixed.contains(&key) {
        return None;
    }
    if pool_risk(ctx, &f.name, &Val::Reg(preg.to_string())) {
        return None;
    }
    let (base, k_gep, terms) = ctx.resolved.get(&key)?.clone();
    if terms.len() != 1 || terms[0].0 != 1 || terms[0].1 != cntreg {
        return None;
    }
    let base_addr = match &base {
        Base::Global(name) => {
            if global_is_const(ctx.module, name) {
                return None;
            }
            *ctx.globals.get(name)?
        }
        // A slot base reads a frame home the match cannot see: decline
        // and let the backend fuse such loops with their homes intact.
        Base::Slot(_, _) => return None,
    };
    let addr = base_addr.wrapping_add(k_gep);
    let (irp, lit) = walk_span_irp(addr, k, ctx.ram_banks, ctx.common_ram)?;
    gep_users
        .entry(preg.to_string())
        .or_default()
        .push(site.clone());
    term_sites.push(site);
    Some((irp, lit))
}

/// The twin of `isel`'s `walk_span_irp` that refuses instead of panicking:
/// every touched byte `[addr, addr + k)` must sit in one FSR window (one
/// IRP half, no SFR hole). The first bank extends through
/// directly-following common RAM.
fn walk_span_irp(
    addr: u16,
    k: u16,
    ram_banks: &[(u16, u16)],
    common_ram: Option<(u16, u16)>,
) -> Option<(bool, u8)> {
    let mut windows: Vec<(u16, u16)> = Vec::new();
    for (i, (lo, hi)) in ram_banks.iter().enumerate() {
        let mut end = hi + 1;
        if i == 0 {
            if let Some((clo, chi)) = common_ram {
                if clo == hi + 1 {
                    end = chi + 1;
                }
            }
        }
        windows.push((*lo, end));
    }
    let win_end = if common_ram.is_some_and(|(clo, chi)| addr >= clo && addr <= chi) {
        windows.first().map(|w| w.1)?
    } else {
        let mut found = None;
        for ((lo, hi), (_, we)) in ram_banks.iter().zip(&windows) {
            if addr >= *lo && addr <= *hi {
                found = Some(*we);
                break;
            }
        }
        found?
    };
    let end = addr.checked_add(k)?;
    if end > win_end {
        return None;
    }
    Some((((addr >> 8) & 1) == 1, ((addr + k) & 0xFF) as u8))
}

/// Mirror the store arm's ROM refusal: a store of a flash address panics
/// there, so the match stays out and lets it panic as before.
fn walk_stores_rom(ctx: &WalkCtx, f: &ir::Func, v: &Val) -> bool {
    match v {
        Val::Global(gg) => global_is_const(ctx.module, gg),
        Val::Reg(r) => matches!(
            ctx.resolved.get(&ssa_key(&f.name, r)),
            Some((Base::Global(gg), _, _)) if global_is_const(ctx.module, gg)
        ),
        _ => false,
    }
}

/// Classify a walk store's value the way the normal store path reads it.
/// Slot reads need only classification here, never the address: the home
/// stays allocated for every value outside the dead set.
fn walk_store_src_ok(ctx: &WalkCtx, f: &ir::Func, v: &Val) -> bool {
    match v {
        Val::Const(_) => true,
        Val::Global(gg) => !global_is_const(ctx.module, gg),
        Val::Reg(r) => {
            if let Some(LoadFold::Direct(gg)) = ctx.folds.loads.get(r) {
                return !global_is_const(ctx.module, gg);
            }
            if ctx.unplaced.contains(r) {
                return false;
            }
            // A pool-rewritten slot reads through the pool reader, not
            // the frame home the walk would name.
            if pool_risk(ctx, &f.name, &Val::Reg(r.clone())) {
                return false;
            }
            true
        }
    }
}

/// Classify an index-as-value store's base: a literal folds into the
/// rebuild, a data global re-reads per iteration, and a register reads
/// its slot. The backend's own dead-load skip (a single-use `@`-global
/// load feeding only the chain, re-read from its global) stays on the
/// emission side: those loads keep their homes here, so the drop set
/// never names a value the planner's twin might still stage.
fn walk_value_base_ok(ctx: &WalkCtx, f: &ir::Func, cntreg: &str, increg: &str, base: Val) -> bool {
    match base {
        Val::Const(_) => true,
        Val::Global(gg) => !global_is_const(ctx.module, &gg),
        Val::Reg(r) => {
            if r == cntreg || r == increg {
                return false;
            }
            if let Some(LoadFold::Direct(gg)) = ctx.folds.loads.get(&r) {
                return !global_is_const(ctx.module, gg);
            }
            if ctx.unplaced.contains(&r) {
                return false;
            }
            if pool_risk(ctx, &f.name, &Val::Reg(r.clone())) {
                return false;
            }
            true
        }
    }
}

/// Region body scan: validates every inst of the header block, records
/// the value chains, and proves the counter feeds only the pattern.
#[allow(clippy::too_many_arguments)]
fn match_body(
    f: &ir::Func,
    b: &ir::Block,
    uses: &HashMap<String, Vec<(String, usize)>>,
    ctx: &WalkCtx,
    cntreg: &str,
    increg: &str,
    up: bool,
    k: u16,
    phi_ii: usize,
    inc_ii: usize,
    latch_ii: usize,
) -> Option<BodyRoles> {
    let by_label: HashMap<&str, &ir::Block> =
        f.blocks.iter().map(|x| (x.label.as_str(), x)).collect();
    let site = |ii: usize| (b.label.clone(), ii);
    let mut ctr_truncs: HashMap<String, (String, usize)> = HashMap::new();
    for (ii, inst) in b.insts.iter().enumerate() {
        if let Inst::Trunc(t) = inst {
            if matches!(&t.val, Val::Reg(r) if r == cntreg) {
                ctr_truncs.insert(t.dst.clone(), site(ii));
            }
        }
    }
    // Masked index links: byte `and`s over the counter (direct or through
    // a trunc of it) against a literal mask. An entry survives only while
    // every use proves to be a masked add; the add arm drops it back to
    // plain ALU otherwise.
    let mut anddefs: HashMap<String, AndDef> = HashMap::new();
    for (ii, inst) in b.insts.iter().enumerate() {
        if let Inst::Bin(q) = inst {
            if q.op != BinOp::And || q.ty.bytes() != 1 {
                continue;
            }
            let mut link: Option<Option<(String, usize)>> = None;
            let mut mask: Option<u8> = None;
            for (side, other) in [(&q.a, &q.b), (&q.b, &q.a)] {
                match side {
                    Val::Reg(r) if r == cntreg => link = Some(None),
                    Val::Reg(r) => {
                        if let Some(t) = ctr_truncs.get(r) {
                            link = Some(Some(t.clone()));
                        }
                    }
                    _ => {}
                }
                if let Val::Const(mm) = other {
                    if (0..=255).contains(mm) {
                        mask = Some(*mm as u8);
                    }
                }
            }
            if let (Some(trunc), Some(mm)) = (link, mask) {
                anddefs.insert(
                    q.dst.clone(),
                    AndDef {
                        mask: mm,
                        trunc,
                        site: site(ii),
                    },
                );
            }
        }
    }
    let mut vadds: HashSet<String> = HashSet::new();
    let mut vadd_bases: HashMap<String, Val> = HashMap::new();
    let mut vadd_truncs: HashMap<String, Option<(String, usize)>> = HashMap::new();
    let mut vadd_masks: HashMap<String, Option<(u8, String)>> = HashMap::new();
    let mut gep_users: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    let mut term_sites: Vec<(String, usize)> = Vec::new();
    // Every walk store consuming a value chain, by chain dst: the finish
    // proof keeps a chain only when all of its uses consume it, so two
    // stores may share one chain but nothing else may read it.
    let mut vadd_consumers: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    // Masked-link dst to the chain dsts reading it, filled as masked adds
    // classify.
    let mut andmap: HashMap<String, Vec<String>> = HashMap::new();
    let mut loads: Vec<(String, (String, usize))> = Vec::new();
    let last = b.insts.len().checked_sub(1)?;
    for (ii, inst) in b.insts.iter().enumerate() {
        if ii == last || ii == phi_ii || ii == latch_ii || ii == inc_ii {
            continue;
        }
        match inst {
            Inst::Phi(p) => {
                if p.ptr {
                    return None;
                }
            }
            Inst::Gep(_) => {}
            Inst::Bin(q) => {
                // A masked index link goes dead iff every use is a masked
                // add (same classifier both arms, so they agree). Anything
                // else refuses: a materialized link would read the counter
                // or trunc home, which the walk never writes.
                if anddefs.contains_key(&q.dst) {
                    let all_masked = uses.get(&q.dst).is_some_and(|u| {
                        !u.is_empty()
                            && u.iter().all(|s| {
                                s.0 == b.label
                                    && by_label.get(s.0.as_str()).is_some_and(|ub| {
                                        ub.insts.get(s.1).is_some_and(|ui| match ui {
                                            Inst::Bin(aq) => walk_masked_vadd(
                                                aq,
                                                cntreg,
                                                &ctr_truncs,
                                                &anddefs,
                                                up,
                                                s.clone(),
                                            )
                                            .is_some(),
                                            _ => false,
                                        })
                                    })
                            })
                    });
                    if !all_masked {
                        return None;
                    }
                    continue;
                }
                match walk_value_add(
                    &q.op,
                    &q.a,
                    &q.b,
                    q.ty.bytes(),
                    cntreg,
                    increg,
                    &ctr_truncs,
                    &anddefs,
                ) {
                    WalkAdd::No => {
                        if let Some((_, tii, base, (mask, and_dst))) =
                            walk_masked_vadd(q, cntreg, &ctr_truncs, &anddefs, up, site(ii))
                        {
                            andmap
                                .entry(and_dst.clone())
                                .or_default()
                                .push(q.dst.clone());
                            vadds.insert(q.dst.clone());
                            vadd_bases.insert(q.dst.clone(), base);
                            vadd_truncs.insert(q.dst.clone(), tii);
                            vadd_masks.insert(q.dst.clone(), Some((mask, and_dst)));
                        } else {
                            if walk_mentions(&q.a, cntreg, increg)
                                || walk_mentions(&q.b, cntreg, increg)
                            {
                                return None;
                            }
                            if !walk_plain_bin(q) {
                                return None;
                            }
                        }
                    }
                    WalkAdd::PostInc => return None,
                    WalkAdd::Direct(base) => {
                        // A down loop visits K..1, so no ascending rebuild
                        // maps onto it: only counter-free down loops walk.
                        if !up {
                            return None;
                        }
                        vadds.insert(q.dst.clone());
                        vadd_bases.insert(q.dst.clone(), base);
                        vadd_truncs.insert(q.dst.clone(), None);
                        vadd_masks.insert(q.dst.clone(), None);
                    }
                    WalkAdd::ViaTrunc(tii, base) => {
                        if !up {
                            return None;
                        }
                        vadds.insert(q.dst.clone());
                        vadd_bases.insert(q.dst.clone(), base);
                        vadd_truncs.insert(q.dst.clone(), Some(tii));
                        vadd_masks.insert(q.dst.clone(), None);
                    }
                }
            }
            Inst::Trunc(t) => {
                if matches!(&t.val, Val::Reg(r) if r == cntreg) {
                    // Counter trunc: feeds a value chain or nothing; the
                    // finish proof pins it. Never emitted.
                } else if matches!(&t.val, Val::Reg(r) if r == increg) {
                    return None;
                }
            }
            Inst::Zext(z) => {
                if walk_mentions(&z.val, cntreg, increg) {
                    return None;
                }
            }
            Inst::Sext(s) => {
                if walk_mentions(&s.val, cntreg, increg) {
                    return None;
                }
            }
            Inst::Icmp(c) => {
                if walk_mentions(&c.a, cntreg, increg) || walk_mentions(&c.b, cntreg, increg) {
                    return None;
                }
            }
            Inst::Select(s) => {
                if walk_mentions(&s.cond, cntreg, increg)
                    || walk_mentions(&s.a, cntreg, increg)
                    || walk_mentions(&s.b, cntreg, increg)
                {
                    return None;
                }
            }
            Inst::Load(l) => {
                if l.ptr.starts_with('@') {
                    continue;
                }
                // Down loops address base+K..base+1 while the
                // re-derivation visits ascending: refuse.
                if !up {
                    return None;
                }
                if l.ty.bytes() != 1 || l.ptr_ty {
                    return None;
                }
                if ctx.unplaced.contains(&l.dst) {
                    return None;
                }
                walk_memop_ok(
                    f,
                    cntreg,
                    &l.ptr,
                    k,
                    ctx,
                    &mut gep_users,
                    &mut term_sites,
                    site(ii),
                )?;
                loads.push((l.dst.clone(), site(ii)));
            }
            Inst::Store(s) => {
                if s.ptr.starts_with('@') {
                    continue;
                }
                // Same descending-address refusal as loads.
                if !up {
                    return None;
                }
                if s.ty.bytes() != 1 {
                    return None;
                }
                if walk_stores_rom(ctx, f, &s.val) {
                    return None;
                }
                if let Val::Reg(v) = &s.val {
                    if vadds.contains(v) {
                        // Shared chains stay walkable while every use
                        // consumes them; the finish proof refuses the rest.
                        if !walk_value_base_ok(ctx, f, cntreg, increg, vadd_bases[v].clone()) {
                            return None;
                        }
                        walk_memop_ok(
                            f,
                            cntreg,
                            &s.ptr,
                            k,
                            ctx,
                            &mut gep_users,
                            &mut term_sites,
                            site(ii),
                        )?;
                        vadd_consumers.entry(v.clone()).or_default().push(site(ii));
                        continue;
                    }
                }
                if walk_mentions(&s.val, cntreg, increg) {
                    return None;
                }
                if !walk_store_src_ok(ctx, f, &s.val) {
                    return None;
                }
                walk_memop_ok(
                    f,
                    cntreg,
                    &s.ptr,
                    k,
                    ctx,
                    &mut gep_users,
                    &mut term_sites,
                    site(ii),
                )?;
            }
            _ => return None,
        }
    }
    // Link sites of masked chains, for the trunc proof. Only links behind
    // a classified chain are ever consulted.
    let and_sites: HashMap<String, (String, usize)> = anddefs
        .iter()
        .map(|(dst, def)| (dst.clone(), def.site.clone()))
        .collect();
    match_finish(
        b,
        uses,
        cntreg,
        increg,
        phi_ii,
        inc_ii,
        latch_ii,
        &vadds,
        &vadd_truncs,
        &vadd_masks,
        &gep_users,
        &term_sites,
        &vadd_consumers,
        &andmap,
        &and_sites,
        &ctr_truncs,
    )?;
    Some(BodyRoles {
        cntreg: cntreg.to_string(),
        vadds,
        anddefs: anddefs.keys().cloned().collect(),
        truncs: ctr_truncs.keys().cloned().collect(),
        loads,
    })
}

/// Walk-loop finish: consumption proofs. Everything the walk drops
/// (value chain, masked links, counter truncs) must read nowhere else;
/// every counter read must sit on the pattern; every GEP must feed only the
/// walk memops that name it. Carried phis may read anywhere in the
/// region; outside it they would see the exiting iteration's backedge
/// copy, one update too many. A value chain may feed several walk
/// stores, but only walk stores.
#[allow(clippy::too_many_arguments)]
fn match_finish(
    b: &ir::Block,
    uses: &HashMap<String, Vec<(String, usize)>>,
    cntreg: &str,
    increg: &str,
    phi_ii: usize,
    inc_ii: usize,
    latch_ii: usize,
    vadds: &HashSet<String>,
    vadd_truncs: &HashMap<String, Option<(String, usize)>>,
    vadd_masks: &HashMap<String, Option<(u8, String)>>,
    gep_users: &HashMap<String, Vec<(String, usize)>>,
    term_sites: &[(String, usize)],
    vadd_consumers: &HashMap<String, Vec<(String, usize)>>,
    andmap: &HashMap<String, Vec<String>>,
    and_sites: &HashMap<String, (String, usize)>,
    ctr_truncs: &HashMap<String, (String, usize)>,
) -> Option<()> {
    // Carried values exit only through body temps: the latch runs the
    // backedge copies on the exiting iteration too, so a carried phi read
    // outside the region would see one update too many.
    for inst in &b.insts {
        if let Inst::Phi(p) = inst {
            if p.dst == cntreg {
                continue;
            }
            if uses
                .get(&p.dst)
                .is_some_and(|u| u.iter().any(|s| s.0 != b.label))
            {
                return None;
            }
        }
    }
    // Value chains feed walk stores alone: every use must consume.
    for dst in vadds {
        let consumers = vadd_consumers.get(dst);
        if uses
            .get(dst)
            .is_some_and(|u| u.iter().any(|s| !consumers.is_some_and(|c| c.contains(s))))
        {
            return None;
        }
    }
    // Masked links feed their chains' adds alone.
    for (and_dst, chain_dsts) in andmap {
        let mut add_sites: Vec<(String, usize)> = Vec::new();
        for d in chain_dsts {
            let found = b.insts.iter().enumerate().find_map(|(ii, i)| match i {
                Inst::Bin(q) if q.dst == *d => Some((b.label.clone(), ii)),
                _ => None,
            })?;
            add_sites.push(found);
        }
        if uses
            .get(and_dst)
            .is_some_and(|u| u.iter().any(|s| !add_sites.contains(s)))
        {
            return None;
        }
    }
    // Counter truncs feed their value-add or, through a masked link, the
    // link itself. Nothing else.
    let mut trunc_sites: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for dst in vadds {
        if !vadd_consumers.contains_key(dst) {
            continue;
        }
        if let Some(t) = vadd_truncs.get(dst).and_then(|x| x.as_ref()) {
            for (treg, ti) in ctr_truncs {
                if ti == t {
                    let reader = match vadd_masks.get(dst).and_then(|x| x.as_ref()) {
                        Some((_, and_dst)) => and_sites.get(and_dst).cloned(),
                        None => None,
                    }
                    .unwrap_or_else(|| {
                        b.insts
                            .iter()
                            .enumerate()
                            .find_map(|(ii, i)| match i {
                                Inst::Bin(q) if q.dst == *dst => Some((b.label.clone(), ii)),
                                _ => None,
                            })
                            .expect("chain known")
                    });
                    trunc_sites.entry(treg.clone()).or_default().push(reader);
                }
            }
        }
    }
    for treg in ctr_truncs.keys() {
        if let Some(u) = uses.get(treg) {
            if !trunc_sites
                .get(treg)
                .is_some_and(|a| u.iter().all(|s| a.contains(s)))
            {
                return None;
            }
        }
    }
    // GEPs feed only the walk memops that name them.
    for inst in &b.insts {
        if let Inst::Gep(gp) = inst {
            let allowed = gep_users.get(&gp.dst);
            let ok = match uses.get(&gp.dst) {
                None => true,
                Some(u) => allowed.is_some_and(|a| u.iter().all(|s| a.contains(s))),
            };
            if !ok {
                return None;
            }
        }
    }
    // Counter regs feed only the pattern: phi, increment, latch, value
    // chain, and walk terms. GEPs emit nothing, so their term reads join
    // the allowed sites.
    let phi_site = (b.label.clone(), phi_ii);
    let inc_site = (b.label.clone(), inc_ii);
    let latch_site = (b.label.clone(), latch_ii);
    let mut cnt_allowed: HashSet<(String, usize)> = HashSet::from([phi_site.clone(), inc_site]);
    let mut inc_allowed: HashSet<(String, usize)> = HashSet::from([phi_site, latch_site]);
    for dst in vadds {
        if let Some(aii) = b.insts.iter().enumerate().find_map(|(ii, i)| match i {
            Inst::Bin(q) if q.dst == *dst => Some((b.label.clone(), ii)),
            _ => None,
        }) {
            cnt_allowed.insert(aii);
        }
        if let Some(Some(t)) = vadd_truncs.get(dst) {
            cnt_allowed.insert(t.clone());
        }
        if let Some(Some((_, and_dst))) = vadd_masks.get(dst) {
            if let Some(s) = and_sites.get(and_dst) {
                cnt_allowed.insert(s.clone());
            }
        }
    }
    for s in term_sites {
        cnt_allowed.insert(s.clone());
    }
    for (ii, inst) in b.insts.iter().enumerate() {
        if matches!(inst, Inst::Gep(_)) {
            cnt_allowed.insert((b.label.clone(), ii));
            inc_allowed.insert((b.label.clone(), ii));
        }
    }
    if uses
        .get(cntreg)
        .is_some_and(|u| !u.iter().all(|s| cnt_allowed.contains(s)))
    {
        return None;
    }
    if uses
        .get(increg)
        .is_some_and(|u| !u.iter().all(|s| inc_allowed.contains(s)))
    {
        return None;
    }
    Some(())
}

/// Call-result regs no use reads (cut 4): the caller copy, the callee
/// retval write, and the fixed retval claim all go away. GEP-propagated
/// reads count as uses, like `home_results`; `asm` reads keep the result.
pub fn dead_call_results(f: &ir::Func) -> HashSet<String> {
    let mut uses: HashMap<String, HashSet<(usize, u16)>> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        for (pos, inst) in b.insts.iter().enumerate() {
            if let ir::Inst::Phi(p) = inst {
                for (v, _) in &p.incoming {
                    let vn = ir::val_name(v);
                    if vn.is_empty() {
                        continue;
                    }
                    uses.entry(vn).or_default().insert((i, u16::MAX));
                }
                continue;
            }
            for v in ir::read_vals(inst) {
                if v.is_empty() {
                    continue;
                }
                uses.entry(v).or_default().insert((i, pos as u16));
            }
        }
    }
    // A GEP's base and term regs are re-read at every load or store
    // through the derived pointer, so those reads count as uses too.
    let mut gep_operands: HashMap<String, Vec<String>> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if let ir::Inst::Gep(g) = inst {
                let mut ops = Vec::new();
                if let ir::GepBase::Reg(r) = &g.base {
                    ops.push(r.clone());
                }
                ops.extend(g.terms.iter().map(|(_, r)| r.clone()));
                gep_operands.insert(g.dst.clone(), ops);
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (dst, ops) in &gep_operands {
            let dst_uses: Vec<(usize, u16)> = uses
                .get(dst)
                .map(|u| u.iter().copied().collect())
                .unwrap_or_default();
            for op in ops {
                if let Some(op_uses) = uses.get_mut(op) {
                    let before = op_uses.len();
                    op_uses.extend(dst_uses.iter().copied());
                    if op_uses.len() != before {
                        changed = true;
                    }
                }
            }
        }
    }
    let mut out = HashSet::new();
    for b in &f.blocks {
        for inst in &b.insts {
            let ir::Inst::Call(c) = inst else { continue };
            let Some(d) = &c.dst else { continue };
            if c.ty.is_none() {
                continue;
            }
            let used = uses.get(d).is_some_and(|u| !u.is_empty());
            if !used {
                out.insert(d.clone());
            }
        }
    }
    out
}

/// Whether any caller uses `func`'s return value (cut 4): a direct call
/// with a live result, or an unknown caller (indirect targets,
/// address-taken functions, entry points, module asm). With no user the
/// callee skips its retval write and the report claims no retval bytes.
pub fn ret_value_used(m: &Module, func: &str) -> bool {
    let Some(f) = m.funcs.iter().find(|x| x.name == func) else {
        return true;
    };
    if f.name == "main" || f.isr || f.naked {
        return true;
    }
    // An unreachable body is a self-branch placeholder with no `Ret` and
    // no live caller (isel rejects a live call): no write, no reader.
    if f.unreachable_body {
        return false;
    }
    if !m.module_asm.is_empty() {
        return true;
    }
    let taken: HashSet<&str> = m
        .funcs
        .iter()
        .flat_map(|g| g.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            ir::Inst::Call(c) if !c.callees.is_empty() => {
                Some(c.callees.iter().map(|s| s.as_str()))
            }
            _ => None,
        })
        .flatten()
        .collect();
    if taken.contains(func) {
        return true;
    }
    for g in &m.funcs {
        let dead = dead_call_results(g);
        for b in &g.blocks {
            for inst in &b.insts {
                if let ir::Inst::Call(c) = inst {
                    if c.func != func || !c.callees.is_empty() {
                        continue;
                    }
                    match &c.dst {
                        Some(d) if !dead.contains(d) => return true,
                        _ => {}
                    }
                }
            }
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{find_value_folds, flash_provenance, resolve_pointers};

    const COPY_IR: &str = "global seed i8\n\
        global bench_mark i8\n\
        global src i8\n\
        global dst i8\n\
        global checksum i8\n\
        fn main(void) ()\n\
        block 0:\n\
        store volatile i8 1 @bench_mark\n\
        br 1\n\
        block 1:\n\
        %2 = phi i16 0 0 %5 1\n\
        %3 = load volatile i8 @seed\n\
        %scevgep = gep @src +0 +1*%2\n\
        %4 = trunc i16 %2 to i8\n\
        %tmp = add i8 %3 %4\n\
        store i8 %tmp %scevgep\n\
        %5 = add i16 %2 1\n\
        %6 = icmp eq i16 %5 32\n\
        br i1 %6 7 1\n\
        block 7:\n\
        %8 = call i16 @memcpy(@dst, @src, i16 32)\n\
        br 9\n\
        block 9:\n\
        %10 = phi i16 0 7 %14 9\n\
        %11 = phi i8 0 7 %13 9\n\
        %scevgep1 = gep @dst +0 +1*%10\n\
        %12 = load i8 %scevgep1\n\
        %13 = add i8 %12 %11\n\
        %14 = add i16 %10 1\n\
        %15 = icmp eq i16 %14 32\n\
        br i1 %15 16 9\n\
        block 16:\n\
        store volatile i8 %13 @checksum\n\
        store volatile i8 2 @bench_mark\n\
        ret void\n\
        fn memcpy(i16) (0=ptr, 1=ptr, 2=i16)\n\
        block 3:\n\
        br 5\n\
        block 4:\n\
        ret i16 0\n\
        block 5:\n\
        %6 = phi i16 %10 5 0 3\n\
        %7 = gep @src +0 +1*%6\n\
        %8 = load i8 %7\n\
        %9 = gep @dst +0 +1*%6\n\
        store i8 %8 %9\n\
        %10 = add i16 %6 1\n\
        %11 = icmp eq i16 %10 32\n\
        br i1 %11 4 5\n";

    fn globals_map() -> HashMap<String, u16> {
        HashMap::from([
            ("seed".to_string(), 0x20),
            ("bench_mark".to_string(), 0x21),
            ("src".to_string(), 0x22),
            ("dst".to_string(), 0x42),
            ("checksum".to_string(), 0x62),
        ])
    }

    fn matches_for(m: &Module, fname: &str, globals: &HashMap<String, u16>) -> Vec<WalkMatch> {
        let f = m.funcs.iter().find(|x| x.name == fname).unwrap();
        let resolved = resolve_pointers(m);
        let safe: HashSet<String> = m
            .globals
            .iter()
            .filter(|g| !g.is_const)
            .map(|g| g.name.clone())
            .collect();
        let folds = find_value_folds(f, m, &resolved, Some(&safe));
        let unplaced = folds.unplaced_pic14(f);
        let prov = flash_provenance(m);
        let ctx = WalkCtx {
            resolved: &resolved,
            folds: &folds,
            unplaced: &unplaced,
            prov: &prov,
            globals,
            ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF), (0x110, 0x16F), (0x190, 0x1EF)],
            common_ram: Some((0x70, 0x7F)),
            module: m,
            pic14: true,
        };
        match_walks(m, f, &ctx)
    }

    fn header<'a>(ms: &'a [WalkMatch], h: &str) -> &'a WalkMatch {
        ms.iter()
            .find(|w| w.header == h)
            .expect("walk header matches")
    }

    /// The fill loop drops its increment, latch compare, and dead index
    /// chain; nothing accumulates there.
    #[test]
    fn fill_loop_drops_latch_and_chain() {
        let m = ir::parse(COPY_IR);
        let globals = globals_map();
        let ms = matches_for(&m, "main", &globals);
        let w = header(&ms, "1");
        assert_eq!(w.k, 32);
        assert!(w.up);
        for r in ["5", "6", "tmp", "4"] {
            assert!(w.dead.contains(r), "{r} takes no home");
        }
        assert!(!w.dead.contains("2"), "counter keeps its home");
        assert!(w.wloads.is_empty());
        assert!(w.acc.is_empty());
    }

    /// The sum loop accumulates the held load into the carried sum: the
    /// load stays in W and the add shares the phi's home.
    #[test]
    fn sum_loop_accumulates() {
        let m = ir::parse(COPY_IR);
        let globals = globals_map();
        let ms = matches_for(&m, "main", &globals);
        let w = header(&ms, "9");
        assert_eq!(w.k, 32);
        for r in ["14", "15"] {
            assert!(w.dead.contains(r), "{r} takes no home");
        }
        assert_eq!(w.latch, "15");
        assert!(w.wloads.contains("12"));
        assert_eq!(w.acc, vec![("13".to_string(), "11".to_string())]);
    }

    /// The copy loop drops its dead increment and latch but parks the
    /// held byte: one FSR forces the park.
    #[test]
    fn memcpy_loop_parks_the_hold() {
        let m = ir::parse(COPY_IR);
        let globals = globals_map();
        let ms = matches_for(&m, "memcpy", &globals);
        assert_eq!(ms.len(), 1);
        let w = &ms[0];
        assert_eq!(w.header, "5");
        for r in ["10", "11"] {
            assert!(w.dead.contains(r), "{r} takes no home");
        }
        assert!(!w.dead.contains("8"), "held byte keeps its home");
        assert!(w.wloads.is_empty());
        assert!(w.acc.is_empty());
    }

    /// Pool-log variant guard (epic-cc#1409): a loop the variant must
    /// decline keeps every home. The same shape in a plain function
    /// still matches.
    #[test]
    fn log_callee_loop_touching_fmt_derivation_declines() {
        const IR: &str = "global src i8\n\
            global dst i8\n\
            fn epic_harness_log(void) (0=ptr)\n\
            block 3:\n\
            br 5\n\
            block 4:\n\
            ret void\n\
            block 5:\n\
            %6 = phi i16 %10 5 0 3\n\
            %d = gep %0 +0\n\
            %7 = gep @src +0 +1*%6\n\
            %8 = load i8 %7\n\
            %9 = gep @dst +0 +1*%6\n\
            store i8 %8 %9\n\
            %10 = add i16 %6 1\n\
            %11 = icmp eq i16 %10 32\n\
            br i1 %11 4 5\n\
            fn log_plain(void) (0=ptr)\n\
            block 3:\n\
            br 5\n\
            block 4:\n\
            ret void\n\
            block 5:\n\
            %6 = phi i16 %10 5 0 3\n\
            %d = gep %0 +0\n\
            %7 = gep @src +0 +1*%6\n\
            %8 = load i8 %7\n\
            %9 = gep @dst +0 +1*%6\n\
            store i8 %8 %9\n\
            %10 = add i16 %6 1\n\
            %11 = icmp eq i16 %10 32\n\
            br i1 %11 4 5\n";
        let m = ir::parse(IR);
        let globals = HashMap::from([("src".to_string(), 0x22), ("dst".to_string(), 0x42)]);
        assert!(
            matches_for(&m, "epic_harness_log", &globals).is_empty(),
            "fmt-derived loop keeps its homes for the variant"
        );
        assert_eq!(
            matches_for(&m, "log_plain", &globals).len(),
            1,
            "same shape still walks outside the log callee"
        );
    }

    /// A span crossing the FSR window keeps every home.
    #[test]
    fn window_crossing_declines() {
        let m = ir::parse(COPY_IR);
        let mut globals = globals_map();
        globals.insert("dst".to_string(), 0x61);
        let ms = matches_for(&m, "main", &globals);
        assert!(
            ms.iter().all(|w| w.header != "9"),
            "sum loop over 0x61..0x81 leaves the window"
        );
    }

    /// Other cores keep every home while this work lands.
    #[test]
    fn non_pic14_matches_nothing() {
        let m = ir::parse(COPY_IR);
        let f = m.funcs.iter().find(|x| x.name == "main").unwrap();
        let resolved = resolve_pointers(&m);
        let folds = ValueFolds::default();
        let unplaced = HashSet::new();
        let prov = flash_provenance(&m);
        let globals = globals_map();
        let ctx = WalkCtx {
            resolved: &resolved,
            folds: &folds,
            unplaced: &unplaced,
            prov: &prov,
            globals: &globals,
            ram_banks: &[(0x20, 0x6F)],
            common_ram: Some((0x70, 0x7F)),
            module: &m,
            pic14: false,
        };
        assert!(match_walks(&m, f, &ctx).is_empty());
    }

    /// The unused call result takes no caller copy and no retval claim.
    #[test]
    fn unused_call_result_is_dead() {
        let m = ir::parse(COPY_IR);
        let main = m.funcs.iter().find(|x| x.name == "main").unwrap();
        assert_eq!(
            dead_call_results(main).into_iter().collect::<Vec<_>>(),
            vec!["8"]
        );
        let memcpy = m.funcs.iter().find(|x| x.name == "memcpy").unwrap();
        assert!(dead_call_results(memcpy).is_empty());
    }

    /// No caller uses memcpy's result, so its retval write goes away.
    #[test]
    fn unused_retval_skips_write() {
        let m = ir::parse(COPY_IR);
        assert!(!ret_value_used(&m, "memcpy"));
        assert!(ret_value_used(&m, "main"));
    }
}
