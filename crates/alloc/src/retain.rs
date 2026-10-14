//! Whole-program callee pointer retention.
//!
//! A frame object's live range may end at its last call argument only when
//! no callee keeps the address alive past the call. A callee that stores the
//! pointer, returns it, or forwards it to a callee that does, keeps reading
//! the bytes after the caller has reused the slot. The walk here answers
//! that per (callee, argument) and is conservative: anything it cannot see
//! through counts as retention.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use ir::{Func, Inst, Module, Val};

/// Pseudo parameter index for the variadic tail of a callee.
const VA_TAIL: usize = usize::MAX;

pub(crate) struct Retention<'m> {
    funcs: HashMap<&'m str, &'m Func>,
    memo: RefCell<HashMap<(&'m str, usize), bool>>,
    visiting: RefCell<HashSet<(&'m str, usize)>>,
}

impl<'m> Retention<'m> {
    pub(crate) fn new(m: &'m Module) -> Self {
        Retention {
            funcs: m.funcs.iter().map(|f| (f.name.as_str(), f)).collect(),
            memo: RefCell::new(HashMap::new()),
            visiting: RefCell::new(HashSet::new()),
        }
    }

    /// Whether argument `i` of `c` may stay reachable after the call returns.
    /// An indirect call, an undefined callee, or an arity mismatch retains.
    pub(crate) fn call_retains(&self, c: &ir::Call, i: usize) -> bool {
        if !c.callees.is_empty() {
            return true;
        }
        match self.funcs.get(c.func.as_str()) {
            Some(&f) => self.arg_retains(f, i),
            None => true,
        }
    }

    fn arg_retains(&self, f: &'m Func, i: usize) -> bool {
        // A naked body is opaque asm: it can keep any param it was handed.
        if f.naked {
            return true;
        }
        let slot = if i < f.params.len() {
            i
        } else if f.variadic {
            VA_TAIL
        } else {
            return true;
        };
        let key = (f.name.as_str(), slot);
        if let Some(&r) = self.memo.borrow().get(&key) {
            return r;
        }
        // Recursion is rejected before allocation; a cycle that slips
        // through is conservatively a retainer.
        if !self.visiting.borrow_mut().insert(key) {
            return true;
        }
        let r = self.walk(f, slot);
        self.visiting.borrow_mut().remove(&key);
        self.memo.borrow_mut().insert(key, r);
        r
    }

    /// Taint the seed (a param, or every `va_arg` result for the variadic
    /// tail) through derived values, then report whether any tainted value
    /// is stored, returned, passed to a retaining callee, or read by a use
    /// the walk does not model. Loads, address compares, and pointer
    /// operands of stores and copies only dereference. A load result is not
    /// tainted: a pointer into the object read back from memory was stored
    /// there first, and every such store (caller or callee) retains.
    fn walk(&self, f: &'m Func, slot: usize) -> bool {
        let mut tainted: HashSet<&str> = HashSet::new();
        if slot == VA_TAIL {
            for b in &f.blocks {
                for inst in &b.insts {
                    if let Inst::VaArg(v) = inst {
                        tainted.insert(v.dst.as_str());
                    }
                }
            }
        } else {
            tainted.insert(f.params[slot].name.as_str());
        }
        if tainted.is_empty() {
            return false;
        }
        let is_t = |t: &HashSet<&str>, v: &Val| matches!(v, Val::Reg(r) if t.contains(r.as_str()));
        let mut changed = true;
        while changed {
            changed = false;
            for b in &f.blocks {
                for inst in &b.insts {
                    let derived: Option<(&str, bool)> = match inst {
                        Inst::Gep(g) => Some((
                            g.dst.as_str(),
                            matches!(&g.base, ir::GepBase::Reg(r) if tainted.contains(r.as_str()))
                                || g.terms.iter().any(|(_, r)| tainted.contains(r.as_str())),
                        )),
                        Inst::Select(s) => {
                            Some((s.dst.as_str(), is_t(&tainted, &s.a) || is_t(&tainted, &s.b)))
                        }
                        Inst::Phi(p) => Some((
                            p.dst.as_str(),
                            p.incoming.iter().any(|(v, _)| is_t(&tainted, v)),
                        )),
                        Inst::Freeze(x) => Some((x.dst.as_str(), is_t(&tainted, &x.val))),
                        Inst::Bin(x) => {
                            Some((x.dst.as_str(), is_t(&tainted, &x.a) || is_t(&tainted, &x.b)))
                        }
                        Inst::Zext(x) => Some((x.dst.as_str(), is_t(&tainted, &x.val))),
                        Inst::Sext(x) => Some((x.dst.as_str(), is_t(&tainted, &x.val))),
                        Inst::Trunc(x) => Some((x.dst.as_str(), is_t(&tainted, &x.val))),
                        Inst::IntToPtr(x) => Some((x.dst.as_str(), is_t(&tainted, &x.val))),
                        _ => None,
                    };
                    if let Some((d, true)) = derived {
                        changed |= tainted.insert(d);
                    }
                }
            }
        }
        for b in &f.blocks {
            for inst in &b.insts {
                let kept = match inst {
                    Inst::Store(s) => is_t(&tainted, &s.val),
                    Inst::Ret(Some((_, v)), _) => is_t(&tainted, v),
                    Inst::Call(c) => {
                        (!c.callees.is_empty() && tainted.contains(c.func.as_str()))
                            || c.args
                                .iter()
                                .enumerate()
                                .any(|(j, a)| is_t(&tainted, &a.val) && self.call_retains(c, j))
                    }
                    Inst::Asm(a) => a
                        .operands
                        .iter()
                        .any(|o| tainted.contains(o.ptr.strip_prefix('%').unwrap_or(&o.ptr))),
                    Inst::Memcpy(mc) => {
                        matches!(&mc.len, ir::MemLen::Reg(v) if is_t(&tainted, v))
                    }
                    Inst::FloatBin(x) => is_t(&tainted, &x.a) || is_t(&tainted, &x.b),
                    Inst::Fcmp(x) => is_t(&tainted, &x.a) || is_t(&tainted, &x.b),
                    Inst::FloatConv(x) => is_t(&tainted, &x.val),
                    _ => false,
                };
                if kept {
                    return true;
                }
            }
        }
        false
    }
}
