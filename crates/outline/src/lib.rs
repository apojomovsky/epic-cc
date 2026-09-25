//! PIC18 code factoring on the final listing (epic-cc#662, docs/44).
//!
//! Runs between `isel-pic18` and `asm`: listing in, listing out, with the
//! per-line source locations carried along. Repeated straight-line runs
//! become shared leaf bodies reached by `RCALL`/`CALL`, and repeated runs
//! ending in `RETURN` keep one copy that the other sites branch to.
//! Allocation is static, so identical text is identical effect, and
//! `CALL`/`RETURN` preserve W, STATUS and BSR: a body runs under its
//! caller's state exactly like the inline copy did.

use ir::SrcLoc;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Knobs for [`factor_with_locs`].
#[derive(Debug, Clone)]
pub struct Options {
    /// Hardware return-stack entries (`Device::stack_depth`).
    pub stack_depth: usize,
    /// The IR call graph's longest chain, which sees indirect-call
    /// candidates the listing cannot.
    pub ir_depth: usize,
    /// Keep every existing address: pad replaced sites with `NOP`s and
    /// put bodies after the code with long forms. Differential tests only.
    pub pad: bool,
    /// Longest candidate run, in instructions.
    pub max_len: usize,
    /// Largest function (in words) whose local bodies are priced with the
    /// 1-word `RCALL`/`BRA`. The assembler relaxes a miss, so this only
    /// steers pricing.
    pub near_limit: u32,
    /// Leave runtime helpers (`__`-prefixed routines other than startup)
    /// alone: they are the inner loops of multiply, divide and float.
    pub skip_runtime: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            stack_depth: 31,
            ir_depth: 0,
            pad: false,
            max_len: 32,
            near_limit: 800,
            skip_runtime: true,
        }
    }
}

const SKIPS: &[&str] = &[
    "DECFSZ", "INCFSZ", "DCFSNZ", "INFSNZ", "BTFSC", "BTFSS", "CPFSEQ", "CPFSGT", "CPFSLT",
    "TSTFSZ",
];
const CONTROL: &[&str] = &[
    "BRA", "BZ", "BNZ", "BC", "BNC", "BN", "BNN", "BOV", "BNOV", "GOTO", "CALL", "RCALL", "RETURN",
    "RETFIE", "RETLW", "SLEEP", "RESET", "POP", "PUSH", "DB", "DW", "DATA",
];
const TERMINATORS: &[&str] = &["RETURN", "RETFIE", "BRA", "GOTO"];
/// Literal-operand instructions: their operand is never a file register.
const LITERAL: &[&str] = &[
    "MOVLW", "ADDLW", "SUBLW", "ANDLW", "IORLW", "XORLW", "MULLW", "MOVLB", "LFSR", "NOP",
];
const TWO_WORD: &[&str] = &["MOVFF", "LFSR", "GOTO", "CALL", "MOVSF", "MOVSS"];
const PREFIX: &str = "__pa";

#[derive(Debug)]
enum Kind {
    Other,
    Label,
    /// A layout barrier: `org`, `.pcltbl`, `.pclalign`, `db`, `end`.
    Barrier {
        org: bool,
    },
    AsmMarker,
    Instr(Ins),
}

#[derive(Debug)]
struct Ins {
    mnem: String,
    key: u32,
    words: u32,
    eligible: bool,
    after_skip: bool,
    in_asm: bool,
    func: usize,
}

struct Listing {
    kinds: Vec<Kind>,
    /// Function names, indexed by `Ins::func`; index 0 is the preamble.
    funcs: Vec<String>,
    func_words: Vec<u32>,
    /// Interned instruction text, indexed by `Ins::key`.
    texts: Vec<String>,
    call_edges: BTreeMap<usize, BTreeSet<usize>>,
    isr_roots: Vec<usize>,
}

fn upper_mnem(body: &str) -> String {
    body.split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

fn normalize(body: &str) -> String {
    let mut parts = body.splitn(2, char::is_whitespace);
    let m = parts.next().unwrap_or("").to_ascii_uppercase();
    let ops: String = parts
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if ops.is_empty() {
        m
    } else {
        format!("{m} {ops}")
    }
}

fn parse_num(tok: &str, equs: &HashMap<String, u32>) -> Option<u32> {
    let t = tok.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u32::from_str_radix(h, 16).ok();
    }
    if let Some(v) = equs.get(t) {
        return Some(*v);
    }
    t.parse().ok()
}

/// Whether a file operand may reach PCL, PCLATH, PCLATU, STKPTR or TOS.
/// Unresolvable operands count as reaching them.
fn touches_stacky(mnem: &str, ops: &str, equs: &HashMap<String, u32>) -> bool {
    if LITERAL.contains(&mnem) || ops.is_empty() {
        return false;
    }
    let toks: Vec<&str> = ops.split(',').map(str::trim).collect();
    let files: Vec<&str> = if mnem == "MOVFF" || mnem == "MOVSF" || mnem == "MOVSS" {
        toks.iter().take(2).copied().collect()
    } else {
        vec![toks[0]]
    };
    for f in files {
        let Some(v) = parse_num(f, equs) else {
            return true;
        };
        // Full 12-bit SFR addresses, plus an 8-bit operand whose low byte
        // aliases them through the access bank or a bank-15 BSR.
        if (0xFF9..=0xFFF).contains(&v) || (0xF9..=0xFF).contains(&v) {
            return true;
        }
    }
    false
}

fn parse(asm: &str) -> Listing {
    let lines: Vec<&str> = asm.lines().collect();
    let mut equs = HashMap::new();
    let mut targets = BTreeSet::new();
    let mut vector_next = false;
    let mut vector_roots = Vec::new();
    for raw in &lines {
        let t = raw.split(';').next().unwrap_or("").trim();
        if let Some(eq) = t.find(" equ ") {
            if let Some(v) = parse_num(&t[eq + 5..], &HashMap::new()) {
                equs.insert(t[..eq].trim().to_string(), v);
            }
            continue;
        }
        let m = upper_mnem(t);
        if m == "CALL" || m == "RCALL" {
            if let Some(x) = t.split_whitespace().nth(1) {
                targets.insert(x.trim_end_matches(',').to_string());
            }
        }
    }
    // Interrupt roots: the body at a vector `org`, or a vector stub's
    // `GOTO` target in priority mode.
    for raw in &lines {
        let t = raw.split(';').next().unwrap_or("").trim();
        if t.is_empty() {
            continue;
        }
        let low = t.to_ascii_lowercase();
        if low.starts_with("org ") {
            let a = parse_num(&t[4..], &equs).unwrap_or(0);
            vector_next = a == 0x0008 || a == 0x0018;
            continue;
        }
        if vector_next {
            if let Some(l) = t.strip_suffix(':') {
                vector_roots.push(l.to_string());
            } else if low.starts_with("goto ") {
                vector_roots.push(t[5..].trim().to_string());
            }
            vector_next = false;
        }
    }
    let mut func_names: BTreeSet<String> = targets.clone();
    func_names.extend(vector_roots.iter().cloned());
    func_names.insert("__start".into());
    func_names.insert("main".into());

    let mut funcs = vec![String::new()];
    let mut func_idx: HashMap<String, usize> = HashMap::new();
    let mut texts = Vec::new();
    let mut intern: HashMap<String, u32> = HashMap::new();
    let mut kinds = Vec::with_capacity(lines.len());
    let mut cur = 0usize;
    let mut in_asm = false;
    let mut prev_skip = false;
    for raw in &lines {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("; --- asm start ---") {
            in_asm = true;
            kinds.push(Kind::AsmMarker);
            continue;
        }
        if trimmed.starts_with("; --- asm end ---") {
            in_asm = false;
            kinds.push(Kind::AsmMarker);
            continue;
        }
        let t = raw.split(';').next().unwrap_or("").trim_end();
        if t.trim().is_empty() || t.contains(" equ ") {
            kinds.push(Kind::Other);
            continue;
        }
        if !t.starts_with(char::is_whitespace) {
            let label = t.trim().trim_end_matches(':').to_string();
            if func_names.contains(&label) {
                cur = *func_idx.entry(label.clone()).or_insert_with(|| {
                    funcs.push(label.clone());
                    funcs.len() - 1
                });
            }
            kinds.push(Kind::Label);
            continue;
        }
        let body = t.trim();
        let m = upper_mnem(body);
        let low = body.to_ascii_lowercase();
        if low.starts_with("org ") {
            kinds.push(Kind::Barrier { org: true });
            continue;
        }
        if m.starts_with('.') || m == "END" || m == "LIST" || m == "RADIX" || m == "DB" || m == "DW"
        {
            kinds.push(Kind::Barrier { org: false });
            continue;
        }
        let norm = normalize(body);
        let key = *intern.entry(norm.clone()).or_insert_with(|| {
            texts.push(norm.clone());
            (texts.len() - 1) as u32
        });
        let ops = body
            .splitn(2, char::is_whitespace)
            .nth(1)
            .unwrap_or("")
            .trim();
        let eligible = !in_asm
            && !SKIPS.contains(&m.as_str())
            && !CONTROL.contains(&m.as_str())
            && !touches_stacky(&m, ops, &equs);
        let words = if TWO_WORD.contains(&m.as_str()) { 2 } else { 1 };
        kinds.push(Kind::Instr(Ins {
            mnem: m.clone(),
            key,
            words,
            eligible,
            after_skip: prev_skip,
            in_asm,
            func: cur,
        }));
        prev_skip = SKIPS.contains(&m.as_str());
    }
    let mut func_words = vec![0u32; funcs.len()];
    let mut call_edges: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (i, k) in kinds.iter().enumerate() {
        if let Kind::Instr(ins) = k {
            func_words[ins.func] += ins.words;
            if ins.mnem == "CALL" || ins.mnem == "RCALL" {
                let t = lines[i].split(';').next().unwrap_or("");
                if let Some(x) = t.split_whitespace().nth(1) {
                    if let Some(&g) = func_idx.get(x.trim_end_matches(',')) {
                        call_edges.entry(ins.func).or_default().insert(g);
                    }
                }
            }
        }
    }
    let isr_roots = vector_roots
        .iter()
        .filter_map(|r| func_idx.get(r).copied())
        .collect();
    Listing {
        kinds,
        funcs,
        func_words,
        texts,
        call_edges,
        isr_roots,
    }
}

/// Longest chain of return addresses below `f` in the listing's direct
/// call graph (0 for a leaf). `None` on a cycle: the listing's function
/// boundaries are inferred from call targets, so a label it cannot see
/// can fold two functions together; the caller then declines to factor.
fn chain(
    l: &Listing,
    f: usize,
    memo: &mut HashMap<usize, usize>,
    seen: &mut Vec<usize>,
) -> Option<usize> {
    if let Some(&d) = memo.get(&f) {
        return Some(d);
    }
    if seen.contains(&f) {
        return None;
    }
    seen.push(f);
    let mut d = 0;
    if let Some(cs) = l.call_edges.get(&f) {
        for &g in cs {
            d = d.max(1 + chain(l, g, memo, seen)?);
        }
    }
    seen.pop();
    memo.insert(f, d);
    Some(d)
}

fn isr_context(l: &Listing) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<usize> = l.isr_roots.clone();
    // ISR-context clones carry the `_isr` suffix (ADR-024) even when only
    // an indirect dispatch reaches them.
    for (i, f) in l.funcs.iter().enumerate() {
        if f.ends_with("_isr") {
            stack.push(i);
        }
    }
    while let Some(f) = stack.pop() {
        if out.insert(f) {
            if let Some(cs) = l.call_edges.get(&f) {
                stack.extend(cs.iter().copied());
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
struct Site {
    start: usize,
    len: usize,
}

struct Pick {
    /// Instruction line indices of every site, first site first.
    sites: Vec<Vec<usize>>,
    tail: bool,
    near: bool,
    func: usize,
    words: u32,
}

/// Factor repeated runs out of a PIC18 listing. Returns the listing
/// unchanged when the stack budget cannot absorb one more leaf level.
pub fn factor_with_locs(
    asm: &str,
    locs: &[Option<SrcLoc>],
    opts: &Options,
) -> (String, Vec<Option<SrcLoc>>) {
    let lines: Vec<&str> = asm.lines().collect();
    let unchanged = || (asm.to_string(), locs.to_vec());
    assert!(
        !lines.iter().any(|l| l.trim_start().starts_with(PREFIX)),
        "outline: listing already defines {PREFIX} labels"
    );
    let l = parse(asm);

    // Budget: the deepest main-line chain gains one leaf level; the
    // interrupt chain stacks on top of it. Both take the IR depth as a
    // floor because indirect calls are invisible in the listing.
    let mut memo = HashMap::new();
    let start = l.funcs.iter().position(|f| f == "__start");
    let Some(main_chain) = start.map_or(Some(0), |s| chain(&l, s, &mut memo, &mut Vec::new()))
    else {
        return unchanged();
    };
    let mut isr_chain: Option<usize> = None;
    for &r in &l.isr_roots {
        let Some(c) = chain(&l, r, &mut memo, &mut Vec::new()) else {
            return unchanged();
        };
        isr_chain = Some(isr_chain.map_or(c, |m| m.max(c)));
    }
    let need =
        main_chain.max(opts.ir_depth) + 1 + isr_chain.map_or(0, |c| 1 + c.max(opts.ir_depth));
    if need > opts.stack_depth {
        return unchanged();
    }
    let isr = isr_context(&l);
    let runtime: BTreeSet<usize> = l
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| opts.skip_runtime && f.starts_with("__") && *f != "__start")
        .map(|(i, _)| i)
        .collect();

    // Basic blocks of instruction line indices.
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    for (i, k) in l.kinds.iter().enumerate() {
        match k {
            Kind::Instr(ins) => {
                cur.push(i);
                if CONTROL.contains(&ins.mnem.as_str()) || SKIPS.contains(&ins.mnem.as_str()) {
                    blocks.push(std::mem::take(&mut cur));
                }
            }
            Kind::Other => {}
            _ => {
                if !cur.is_empty() {
                    blocks.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    let ins = |i: usize| match &l.kinds[i] {
        Kind::Instr(x) => x,
        _ => unreachable!(),
    };
    let is_return =
        |x: &Ins| l.texts[x.key as usize] == "RETURN" || l.texts[x.key as usize] == "RETURN 0";

    // Candidates: key sequence -> sites (block, start, len), in listing order.
    let mut cands: HashMap<Vec<u32>, Vec<(usize, Site)>> = HashMap::new();
    for (b, blk) in blocks.iter().enumerate() {
        for s in 0..blk.len() {
            let first = ins(blk[s]);
            if first.after_skip
                || first.in_asm
                || isr.contains(&first.func)
                || runtime.contains(&first.func)
            {
                continue;
            }
            let mut key = Vec::new();
            for n in 0..opts.max_len.min(blk.len() - s) {
                let x = ins(blk[s + n]);
                key.push(x.key);
                if is_return(x) {
                    if n >= 1 {
                        cands.entry(key.clone()).or_default().push((
                            b,
                            Site {
                                start: s,
                                len: n + 1,
                            },
                        ));
                    }
                    break;
                }
                if !x.eligible {
                    break;
                }
                if n >= 1 {
                    cands.entry(key.clone()).or_default().push((
                        b,
                        Site {
                            start: s,
                            len: n + 1,
                        },
                    ));
                }
            }
        }
    }
    let mut live: Vec<(Vec<u32>, Vec<(usize, Site)>)> =
        cands.into_iter().filter(|(_, v)| v.len() >= 2).collect();
    // Deterministic iteration regardless of hash order.
    live.sort_by(|a, b| {
        a.1[0]
            .1
            .start
            .cmp(&b.1[0].1.start)
            .then(a.1[0].0.cmp(&b.1[0].0))
            .then(a.0.cmp(&b.0))
    });

    let spots = placement_spots(&l);
    let mut used: Vec<Vec<bool>> = blocks.iter().map(|b| vec![false; b.len()]).collect();
    let mut picks: Vec<Pick> = Vec::new();
    loop {
        let mut best: Option<(i64, usize, Vec<(usize, Site)>, bool, bool, usize, u32)> = None;
        for (ci, (_, sites)) in live.iter().enumerate() {
            let mut picked: Vec<(usize, Site)> = Vec::new();
            for &(b, s) in sites {
                if used[b][s.start..s.start + s.len].iter().any(|&u| u) {
                    continue;
                }
                if let Some(&(pb, ps)) = picked.last() {
                    if pb == b && ps.start + ps.len > s.start {
                        continue;
                    }
                }
                picked.push((b, s));
            }
            if picked.len() < 2 {
                continue;
            }
            let (b0, s0) = picked[0];
            let tail = is_return(ins(blocks[b0][s0.start + s0.len - 1]));
            let words: u32 = (0..s0.len)
                .map(|j| ins(blocks[b0][s0.start + j]).words)
                .sum();
            let funcs: BTreeSet<usize> = picked
                .iter()
                .map(|&(b, s)| ins(blocks[b][s.start]).func)
                .collect();
            let f0 = *funcs.iter().next().unwrap();
            let local = !opts.pad && funcs.len() == 1 && l.func_words[f0] <= opts.near_limit;
            let near = local && (tail || spots.contains_key(&f0));
            let k = picked.len() as i64;
            let w = words as i64;
            let gain = if tail {
                let j = if near { 1 } else { 2 };
                (k - 1) * (w - j)
            } else {
                let c = if near { 1 } else { 2 };
                k * w - (k * c + w + 1)
            };
            if gain > 0 && best.as_ref().map_or(true, |bst| gain > bst.0) {
                best = Some((gain, ci, picked, tail, near, f0, words));
            }
        }
        let Some((_, ci, picked, tail, near, func, words)) = best else {
            break;
        };
        for &(b, s) in &picked {
            for j in s.start..s.start + s.len {
                used[b][j] = true;
            }
        }
        picks.push(Pick {
            sites: picked
                .iter()
                .map(|&(b, s)| blocks[b][s.start..s.start + s.len].to_vec())
                .collect(),
            tail,
            near,
            func,
            words,
        });
        live.swap_remove(ci);
    }
    if picks.is_empty() {
        return unchanged();
    }
    rewrite(&l, &lines, locs, &picks, &spots, opts)
}

/// Function -> line index of its last unconditional terminator, where a
/// local body cannot land on a fall-through path. A function whose end
/// falls through, sits in an asm region, or is followed by an `org`
/// before the next instruction gets no spot.
fn placement_spots(l: &Listing) -> BTreeMap<usize, usize> {
    let mut last_instr: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, k) in l.kinds.iter().enumerate() {
        if let Kind::Instr(x) = k {
            last_instr.insert(x.func, i);
        }
    }
    let mut spots = BTreeMap::new();
    for (&f, &i) in &last_instr {
        let Kind::Instr(x) = &l.kinds[i] else {
            continue;
        };
        if x.in_asm || !TERMINATORS.contains(&x.mnem.as_str()) || f == 0 {
            continue;
        }
        let mut blocked = false;
        for k in &l.kinds[i + 1..] {
            match k {
                Kind::Instr(_) => break,
                Kind::Barrier { org: true } => {
                    blocked = true;
                    break;
                }
                _ => {}
            }
        }
        if !blocked {
            spots.insert(f, i);
        }
    }
    spots
}

fn rewrite(
    l: &Listing,
    lines: &[&str],
    locs: &[Option<SrcLoc>],
    picks: &[Pick],
    spots: &BTreeMap<usize, usize>,
    opts: &Options,
) -> (String, Vec<Option<SrcLoc>>) {
    let loc = |i: usize| locs.get(i).cloned().flatten();
    // Per original line: replacement lines, or deletion.
    let mut replace: HashMap<usize, Vec<(String, Option<SrcLoc>)>> = HashMap::new();
    let mut deleted: BTreeSet<usize> = BTreeSet::new();
    let mut prefix_label: HashMap<usize, String> = HashMap::new();
    let mut after: BTreeMap<usize, Vec<(String, Option<SrcLoc>)>> = BTreeMap::new();
    let mut far: Vec<(String, Option<SrcLoc>)> = Vec::new();
    let mut site_first: HashMap<usize, usize> = HashMap::new();
    for (n, p) in picks.iter().enumerate() {
        let name = format!("{PREFIX}{n}");
        let (rest, op) = if p.tail {
            prefix_label.insert(p.sites[0][0], name.clone());
            let op = if p.near && !opts.pad { "BRA" } else { "GOTO" };
            (&p.sites[1..], op)
        } else {
            let first = &p.sites[0];
            let mut body = vec![(format!("{name}:"), None)];
            for &i in first {
                body.push((format!("    {}", lines[i].trim()), loc(i)));
            }
            body.push(("    RETURN".to_string(), None));
            if p.near && !opts.pad {
                after.entry(spots[&p.func]).or_default().extend(body);
            } else {
                far.extend(body);
            }
            let op = if p.near && !opts.pad { "RCALL" } else { "CALL" };
            (&p.sites[..], op)
        };
        let op_words = if op == "CALL" || op == "GOTO" { 2 } else { 1 };
        for site in rest {
            let mut out = vec![(format!("    {op} {name}"), loc(site[0]))];
            if opts.pad {
                for _ in op_words..p.words {
                    out.push(("    NOP".to_string(), None));
                }
            }
            replace.insert(site[0], out);
            for &i in &site[1..] {
                deleted.insert(i);
                site_first.insert(i, site[0]);
            }
        }
    }
    // A spot deleted by a tail merge moves to its site's jump.
    let mut moved: BTreeMap<usize, Vec<(String, Option<SrcLoc>)>> = BTreeMap::new();
    for (i, body) in after {
        let at = site_first.get(&i).copied().unwrap_or(i);
        moved.entry(at).or_default().extend(body);
    }
    let end = l
        .kinds
        .iter()
        .rposition(|k| matches!(k, Kind::Barrier { org: false }))
        .filter(|&i| lines[i].trim().eq_ignore_ascii_case("end"));
    let mut out = String::with_capacity(lines.len() * 16);
    let mut out_locs = Vec::with_capacity(lines.len());
    let mut push = |t: &str, lc: Option<SrcLoc>, out: &mut String| {
        out.push_str(t);
        out.push('\n');
        out_locs.push(lc);
    };
    for (i, raw) in lines.iter().enumerate() {
        if Some(i) == end {
            for (t, lc) in far.drain(..) {
                push(&t, lc, &mut out);
            }
        }
        if let Some(lab) = prefix_label.get(&i) {
            push(&format!("{lab}:"), None, &mut out);
        }
        if deleted.contains(&i) {
            continue;
        }
        if let Some(rep) = replace.get(&i) {
            for (t, lc) in rep {
                push(t, lc.clone(), &mut out);
            }
        } else {
            push(raw, loc(i), &mut out);
        }
        if let Some(body) = moved.get(&i) {
            for (t, lc) in body {
                push(t, lc.clone(), &mut out);
            }
        }
    }
    for (t, lc) in far.drain(..) {
        push(&t, lc, &mut out);
    }
    (out, out_locs)
}

/// [`factor_with_locs`] without source locations.
pub fn factor(asm: &str, opts: &Options) -> String {
    factor_with_locs(asm, &[], opts).0
}
