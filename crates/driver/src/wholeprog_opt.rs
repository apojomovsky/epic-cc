//! Whole-program IR cleanup: runs LLVM `opt` over the `llvm-link`-merged
//! module before it reaches `irparse`.
//!
//! Each `.c` file is compiled by clang **alone**: clang never sees that
//! `epic_tick_init(fosc_hz)`'s only caller always passes the literal
//! `FOSC_HZ`, so the whole nested-loop period search in the file-local
//! helper it calls survives untouched into every TU's `.ll`. But by the
//! time `llvm-link` has merged every TU, the whole call graph, and every
//! cross-TU constant argument, is visible in one module. Nothing after
//! `llvm-link` ever re-derives that, so a whole-program compiler that
//! skips this step is leaving exactly the constant-folding a linking,
//! whole-program compiler exists to do on the table (epic-cc#193).
//!
//! The pass list is deliberately narrow, chosen to preserve the property
//! the overlay allocator (`alloc`) depends on: a function's locals are
//! only live between its call and return, so two functions that are never
//! simultaneously live can share RAM. A generic `-O2` inlines aggressively
//! enough to merge callee locals permanently into `main`'s frame (`main`
//! never returns, so nothing it ever contains can be reclaimed), breaking
//! the RAM reuse. None of
//! `internalize`/`ipsccp`/`instcombine`/`simplifycfg`/`dce` inline a
//! multi-call-site function across a call boundary, so call-graph shape
//! (and the RAM reuse it buys) is preserved:
//!   - `internalize`: every function except `main` and the interrupt
//!     handler(s) becomes internal-linkage. This is a real whole-program
//!     compile (ADR-002, no external linker, no separate objects survive
//!     past this point), so nothing outside this module can ever call
//!     them; internalizing is what lets the passes below see and use a
//!     function's complete call-site set.
//!   - `ipsccp`: interprocedural sparse conditional constant propagation.
//!     Once a function is internal, this specializes on a constant
//!     argument shared by every call site (or folds a dead branch from a
//!     config constant) without inlining the callee's body anywhere.
//!   - `instcombine`/`simplifycfg`/`dce`: local cleanup of the constants
//!     `ipsccp` exposes (dead branches, now-constant arithmetic).
//!   - `always-inline` (conditional, `always_inline_candidates`/
//!     `mark_always_inline`): folds only the functions with exactly one
//!     direct call site, into an ordinary (non-`main`, non-ISR) caller,
//!     never referenced any other way. That caller already reclaims its
//!     frame on return, so the callee's locals do too, same as before the
//!     fold; this is free on every axis, no RAM/flash trade, unlike inlining
//!     into `main`/an ISR root (that shape trades flash for permanent RAM
//!     and needs the `-O2` "aggressive" tier's explicit budget check).
//!     Marks candidates with `alwaysinline` textually, so LLVM's
//!     `always-inline` pass folds exactly this set and nothing else, no
//!     cost-model guessing. Skipped entirely (falls back to the base
//!     `PASSES` list) when no candidates are found (epic-cc#205,
//!     epic-cc#204).
//!
//! On top of that baseline, two further independent fixes stack:
//! consolidating `printf`'s per-call-site literal staging buffer into one
//! shared buffer (epic-hal#123, epic-hal#124), and the always-inline pass
//! above. This pass never inlines into `main` or an ISR by design; each
//! fold shortens one link, which compounds along chains that pass through
//! several of them (epic-cc#205, epic-cc#206).
//!
//! `loop-reduce` (LSR) rides in the baseline list too. It leaves call-graph
//! shape alone, so the RAM-reuse invariant above still holds. It rewrites
//! walked pointers as scalar-evolution phis (`%lsr.iv = phi ptr ...`); the
//! PIC14 backend used to miscompile those, reading an `alloca`'s bytes
//! where the walk needed its frame address, which is fixed (epic-cc#647),
//! so both cores run it now.
//! After the base list, a guarded second run tries to fold an
//! all-constant search loop (the `compute_period` shape, epic-cc#1212):
//! it removes the `llvm.loop.unroll.disable` mark only from loops
//! holding a constant divide, unrolls at a raised threshold, and
//! cleans up. The result ships only when every marked loop's
//! function shrinks and no other function grows, so sibling runtime
//! loops stay rolled. Skipped at `-O0`: control flow untouched.
use crate::cli::OptLevel;

use std::path::Path;
use std::process::Command;
/// The curated, RAM-safe pass list (see module docs for why each pass is
/// here and none of them inline across a call boundary).
///
/// `loop-reduce` (LSR) is the largest measured density lever left: it
/// hoists a struct-array stride multiply out of the loop. On PIC18 (which
/// has `MULWF`) that is 49 words on `bench-struct-scan` and 132 on
/// `hal-pic18-menu-demo-18f4550`; on PIC14, which has no hardware multiply
/// and re-emits a shift-add chain per access, hoisting it is worth an order
/// of magnitude more (roughly -494 words on the same scan shape). It
/// rewrites walked pointers as scalar-evolution phis (`%lsr.iv = phi ptr
/// ...`), a shape PIC14 used to miscompile by reading an `alloca`'s bytes
/// in place of its frame address (epic-cc#647).
const PASSES: &str =
    "internalize,ipsccp,instcombine,simplifycfg,dce,loop-reduce,instcombine,simplifycfg,dce";
/// The `-O0` pipeline: whole-program constants folded, control flow
/// untouched. Skipping `opt` entirely was measured and rejected: the menu
/// demo then reaches 18227 words and no longer fits the 18F4550's 16384
/// words of flash, so an `-O0` that builds nothing real is no use to the
/// debugger track. `internalize`/`ipsccp` plus local cleanup only fold
/// cross-TU constant arguments (the `FOSC_HZ` period search is the big
/// one); they never restructure a loop, fold a call, or share a body, so
/// the IR keeps the source's control flow for stepping and bisection.
const PASSES_O0: &str = "internalize,ipsccp,instcombine,simplifycfg,dce";

/// Symbols `internalize` must never touch:
///
/// - The program entry: `main`, or its C++-mangled form `_Z4mainv` (what
///   `int main()` lowers to on the `.cpp` path, epic-cc#457), and every
///   `msp430_intrcc` interrupt handler (the vector table's entry points,
///   `irparse` identifies them the same way, by that calling-convention
///   token on the `define` line).
/// - Every **variadic** function (a `(...)` parameter list). Load-bearing:
///   internalizing a single-call-site variadic function lets `ipsccp`
///   replace every use of its named format-string parameter with the
///   caller's literal, sound as a pure value substitution, but our
///   `llvm.va_start` lowering locates the first vararg relative to that
///   parameter's own frame slot, and a parameter with no remaining SSA
///   uses doesn't reliably get one allocated. The vararg walk silently
///   corrupts (one format byte read as 0), no panic. So the whole function
///   stays external, since external-linkage arguments are never specialized
///   by `ipsccp` (epic-cc#131).
/// - Every **mutable global variable** (LLVM `global`, not `constant`).
///   Also load-bearing: internalizing a plain (non-`const`, non-`volatile`)
///   global that nothing in the compiled program ever stores to lets
///   `ipsccp` read it as permanently equal to its zero-initializer. Correct
///   for a truly closed program, but epic-cc's own e2e harness (and real
///   embedded code with a memory-mapped input) writes such globals from
///   outside the compiled image, a channel no IR-level analysis can see. A
///   `constant` global has no such hazard, nothing ever writes it by
///   construction, so those stay eligible (epic-cc#133).

/// Entry-point name: plain `main`, or the C++-mangled `_Z4mainv` the `.cpp`
/// path emits for `int main()` (epic-cc#457). Both match sites share it so
/// the mangled entry gets the same `internalize` protection as `main`.
fn is_entry_name(name: &str) -> bool {
    name == "main" || name == "_Z4mainv"
}

fn public_api(ll_text: &str) -> Vec<String> {
    let mut api = Vec::new();
    for line in ll_text.lines() {
        if line.starts_with("define") {
            let Some(at) = line.find('@') else { continue };
            let name_start = at + 1;
            let name_end = line[name_start..]
                .find(|c: char| c == '(' || c.is_whitespace())
                .map(|i| name_start + i)
                .unwrap_or(line.len());
            let name = &line[name_start..name_end];
            let variadic =
                line.contains(", ...)") || line[name_end..].trim_start().starts_with("(...)");
            if is_entry_name(name)
                || line.split_whitespace().any(|t| t == "msp430_intrcc")
                || variadic
            {
                api.push(name.to_string());
            }
        } else if line.starts_with('@') && line.split_whitespace().any(|t| t == "global") {
            // A top-level `@name = ... global ...` declaration: mutable
            // storage, preserved. `constant` declarations (and anything
            // else, aliases, ifuncs) fall through and stay eligible.
            let name_end = line[1..]
                .find(|c: char| c == '=' || c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(line.len());
            api.push(line[1..name_end].to_string());
        }
    }
    api
}

/// One `define`d function's name, entry-point status (the C `main` or the
/// C++ `_Z4mainv`, or a `msp430_intrcc` ISR), and line range `[start, end]`
/// (the `define` line through its closing `}`, which clang always emits
/// alone on its own unindented line, never sharing a line with a
/// struct-literal `}`).
struct FuncSpan {
    name: String,
    is_entry: bool,
    start: usize,
    end: usize,
}

fn function_spans(lines: &[&str]) -> Vec<FuncSpan> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("define") {
            if let Some(at) = line.find('@') {
                let name_start = at + 1;
                let name_end = line[name_start..]
                    .find(|c: char| c == '(' || c.is_whitespace())
                    .map(|o| name_start + o)
                    .unwrap_or(line.len());
                let name = line[name_start..name_end].to_string();
                let is_entry =
                    is_entry_name(&name) || line.split_whitespace().any(|t| t == "msp430_intrcc");
                let start = i;
                let mut j = i + 1;
                while j < lines.len() && lines[j] != "}" {
                    j += 1;
                }
                out.push(FuncSpan {
                    name,
                    is_entry,
                    start,
                    end: j,
                });
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Every function whose `define` line carries `noinline`, directly or via
/// its referenced `attributes #N = { ... }` group. `noinline` and
/// `alwaysinline` are LLVM-incompatible on the same function (`opt`
/// aborts: "Attributes 'noinline and alwaysinline' are incompatible!",
/// hit on this crate's own `float_e2e` fixture, whose `mk`/`pick` are
/// deliberately `__attribute__((noinline))` to keep their byval/sret
/// call sites out-of-line for the test to observe). `noinline` is also
/// never accidental, someone chose it for a reason this pass can't see,
/// so exclude rather than override.
fn noinline_functions(lines: &[&str], funcs: &[FuncSpan]) -> std::collections::HashSet<String> {
    let mut groups: std::collections::HashMap<&str, &str> = Default::default();
    for line in lines {
        let Some(rest) = line.strip_prefix("attributes ") else {
            continue;
        };
        let Some(id_end) = rest.find(|c: char| c.is_whitespace()) else {
            continue;
        };
        if let (Some(open), Some(close)) = (line.find('{'), line.rfind('}')) {
            if open < close {
                groups.insert(&rest[..id_end], &line[open + 1..close]);
            }
        }
    }
    funcs
        .iter()
        .filter(|f| {
            let line = lines[f.start];
            let bare = line.split_whitespace().any(|t| t == "noinline");
            let via_group = line
                .split_whitespace()
                .filter(|t| t.starts_with('#') && t[1..].chars().all(|c| c.is_ascii_digit()))
                .any(|id| {
                    groups
                        .get(id)
                        .is_some_and(|attrs| attrs.split_whitespace().any(|t| t == "noinline"))
                });
            bare || via_group
        })
        .map(|f| f.name.clone())
        .collect()
}

/// Functions safe to unconditionally fold into their one caller: exactly
/// one direct `call`/`tail call` site anywhere in the module, never
/// referenced any other way (no stored function pointer, no vtable-style
/// global initializer entry, ruled out so folding cannot silently drop a
/// second, indirect path to it), not already `noinline` (see
/// `noinline_functions`), and that one caller is an ordinary function,
/// not `main` or an ISR root.
///
/// The last condition is the one that matters: `main` and ISR entry
/// frames never return, so anything folded into them becomes permanent,
/// unreclaimable RAM regardless of call-site count (epic-cc#205, #206).
/// A single-call-site fold into an ordinary function costs nothing on
/// any axis, its locals still get reclaimed when that function returns,
/// same as before, so this list is unconditionally safe to always-inline,
/// no RAM/flash trade to weigh, unlike a fold into `main`/an ISR (that
/// shape is the `-O2` "aggressive" tier's job, not this one, epic-cc#204).
///
/// With `aggressive`, entry callers count too, but only for small
/// callees: at most `MAX_ENTRY_INLINE_LINES` IR lines. The fold still
/// duplicates the body exactly once (one call site), so flash barely
/// moves, while the callee's frame becomes permanent RAM in a root that
/// never returns. The line cap bounds that RAM cost to small helpers;
/// a large single-use driver stays out of line on purpose.
const MAX_ENTRY_INLINE_LINES: usize = 100;
fn always_inline_candidates(ll_text: &str, aggressive: bool) -> Vec<String> {
    let lines: Vec<&str> = ll_text.lines().collect();
    let funcs = function_spans(&lines);
    let defined: std::collections::HashSet<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    let noinline = noinline_functions(&lines, &funcs);

    // caller function index for each line (None outside any function body,
    // e.g. a global initializer).
    let mut line_owner: Vec<Option<usize>> = vec![None; lines.len()];
    for (fi, f) in funcs.iter().enumerate() {
        for entry in line_owner.iter_mut().take(f.end + 1).skip(f.start) {
            *entry = Some(fi);
        }
    }

    // Each `define` line's own function name, keyed by line index: it's
    // a declaration, not a reference, so the scan below must not treat
    // it as a call or as address-taken use of itself.
    let own_name_at: std::collections::HashMap<usize, &str> =
        funcs.iter().map(|f| (f.start, f.name.as_str())).collect();

    let mut call_sites: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    let mut address_taken: std::collections::HashSet<&str> = Default::default();

    for (li, line) in lines.iter().enumerate() {
        let mut rest = *line;
        let mut offset = 0usize;
        while let Some(pos) = rest.find('@') {
            let abs = offset + pos;
            let name_start = abs + 1;
            let name_end = line[name_start..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '$'))
                .map(|o| name_start + o)
                .unwrap_or(line.len());
            let name = &line[name_start..name_end];
            if defined.contains(name) && own_name_at.get(&li) != Some(&name) {
                // `call`/`tail call` puts a return type between the
                // keyword and `@name`, so check for the token anywhere
                // earlier on the line, not immediately before `@name`.
                let before = &line[..abs];
                let after = line[name_end..].trim_start();
                let is_direct_call =
                    after.starts_with('(') && before.split_whitespace().any(|t| t == "call");
                if is_direct_call {
                    call_sites
                        .entry(name)
                        .or_default()
                        .push(line_owner[li].unwrap_or(usize::MAX));
                } else {
                    address_taken.insert(name);
                }
            }
            let advance = (name_end - offset).max(pos + 1);
            rest = &line[offset + advance..];
            offset += advance;
        }
    }

    funcs
        .iter()
        .filter(|f| !f.is_entry)
        .filter(|f| !address_taken.contains(f.name.as_str()))
        .filter(|f| !noinline.contains(f.name.as_str()))
        .filter_map(|f| {
            let sites = call_sites.get(f.name.as_str())?;
            if sites.len() != 1 {
                return None;
            }
            let caller = sites[0];
            if caller == usize::MAX {
                return None;
            }
            if funcs[caller].is_entry {
                if !aggressive {
                    return None;
                }
                if f.end - f.start + 1 > MAX_ENTRY_INLINE_LINES {
                    return None;
                }
            }
            Some(f.name.clone())
        })
        .collect()
}

/// Textually add the `alwaysinline` attribute to each candidate's
/// `define` line, so `always-inline` in `PASSES` folds exactly this set
/// and nothing else, no cost-model guessing.
fn mark_always_inline(ll_text: &str, candidates: &[String]) -> String {
    if candidates.is_empty() {
        return ll_text.to_string();
    }
    let set: std::collections::HashSet<&str> = candidates.iter().map(String::as_str).collect();
    let mut out = String::with_capacity(ll_text.len());
    for line in ll_text.lines() {
        if line.starts_with("define") {
            if let Some(at) = line.find('@') {
                let name_start = at + 1;
                let name_end = line[name_start..]
                    .find(|c: char| c == '(' || c.is_whitespace())
                    .map(|o| name_start + o)
                    .unwrap_or(line.len());
                if set.contains(&line[name_start..name_end]) {
                    // `local_unnamed_addr` has a fixed position right after
                    // the parameter list, and metadata (` !dbg !9`, ...)
                    // must be the last thing before `{`, so insert right
                    // before the first metadata attachment (else right
                    // before the brace); `opt` otherwise errors
                    // "expected '{' in function body".
                    if let Some(brace) = line.rfind('{') {
                        let head = &line[..brace];
                        let insert_at = head
                            .match_indices('!')
                            .find(|(i, _)| {
                                *i > 0
                                    && head.as_bytes()[i - 1] == b' '
                                    && head[i + 1..].starts_with(|c: char| c.is_alphabetic())
                            })
                            .map(|(i, _)| i)
                            .unwrap_or(brace);
                        out.push_str(&line[..insert_at]);
                        out.push_str("alwaysinline ");
                        out.push_str(&line[insert_at..]);
                        out.push('\n');
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Cleanup after a forced unroll: `sccp`/`adce` collapse the unrolled
/// constant chain that plain `dce` leaves behind.
const FOLD_PASSES: &str =
    "loop-unroll,instcombine,simplifycfg,sccp,adce,instcombine,simplifycfg,dce";
/// Raised unroll threshold for the fold run. The default (a few hundred)
/// stops just short of the 3x16x2 search body; this clears it while the
/// per-function guard in `run` bounds what can actually ship.
const FOLD_UNROLL_THRESHOLD: &str = "1024";

/// One divide or remainder with a constant operand (`udiv i32 5000, %pq`).
fn is_const_div_line(t: &str) -> bool {
    ["udiv", "sdiv", "urem", "srem"].iter().any(|op| {
        let Some(pos) = t.find(&format!(" {op} ")) else {
            return false;
        };
        let rest = &t[pos + op.len() + 2..];
        let mut parts = rest.splitn(2, char::is_whitespace);
        let _ty = parts.next();
        let Some(operands) = parts.next() else {
            return false;
        };
        operands.split(',').any(|o| {
            let o = o.trim();
            o.strip_prefix('-')
                .unwrap_or(o)
                .starts_with(|c: char| c.is_ascii_digit())
        })
    })
}

/// A basic block in one function: successor labels, the loop ids on
/// its branches, and whether any instruction divides by or of a
/// constant.
struct BlockInfo {
    name: String,
    succs: Vec<String>,
    loop_ids: Vec<String>,
    has_const_div: bool,
}

/// The `label %name` targets in one terminator line.
fn label_targets(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = t;
    while let Some(pos) = rest.find("label %") {
        let tok = &rest[pos + "label %".len()..];
        let name: String = tok
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
            .collect();
        if !name.is_empty() {
            out.push(name.clone());
        }
        rest = &tok[name.len().max(1)..];
    }
    out
}

/// The `!llvm.loop !N` ids attached to one line.
fn loop_ids_on(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = t;
    while let Some(pos) = rest.find("!llvm.loop !") {
        let num = &rest[pos + "!llvm.loop !".len()..];
        let id: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
        if id.is_empty() {
            break;
        }
        out.push(id.clone());
        rest = &num[id.len()..];
    }
    out
}

/// Basic blocks of one function span. A label line is a head before
/// any `;` comment that ends in `:` with no whitespace or `=` in it;
/// the entry block keeps the empty name.
fn blocks_of(lines: &[&str], span: &FuncSpan) -> Vec<BlockInfo> {
    let mut blocks = vec![BlockInfo {
        name: String::new(),
        succs: Vec::new(),
        loop_ids: Vec::new(),
        has_const_div: false,
    }];
    for line in &lines[span.start + 1..span.end] {
        let t = line.trim();
        let head = t.split(';').next().unwrap_or("").trim();
        if head.ends_with(':')
            && !head[..head.len() - 1].contains(|c: char| c.is_whitespace() || c == '=')
        {
            blocks.push(BlockInfo {
                name: head[..head.len() - 1].to_string(),
                succs: Vec::new(),
                loop_ids: Vec::new(),
                has_const_div: false,
            });
            continue;
        }
        let b = blocks.last_mut().expect("entry block exists");
        b.succs.extend(label_targets(t));
        b.loop_ids.extend(loop_ids_on(t));
        if is_const_div_line(t) {
            b.has_const_div = true;
        }
    }
    blocks
}

/// Loop metadata ids in the nest of a constant divide: loops whose
/// natural loop holds a divide, and loops nested inside or enclosing
/// one. The natural loop of a back edge latch to header is the header
/// and every block reaching the latch without passing the header.
/// The whole nest must unroll to evaluate the search; a sequential
/// sibling loop is disjoint and stays rolled.
fn const_div_loops(lines: &[&str], span: &FuncSpan) -> (Vec<String>, bool) {
    let blocks = blocks_of(lines, span);
    let index: std::collections::HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (i, b) in blocks.iter().enumerate() {
        for s in &b.succs {
            if let Some(&j) = index.get(s.as_str()) {
                preds[j].push(i);
            }
        }
    }
    // Dominators: a back edge's header dominates its latch. Reachability
    // alone also admits forward successors inside a shared outer cycle,
    // which would invent loops that LLVM does not see.
    let n_blocks = blocks.len();
    let mut dom = vec![vec![true; n_blocks]; n_blocks];
    if n_blocks > 0 {
        dom[0] = vec![false; n_blocks];
        dom[0][0] = true;
        let mut changed = true;
        while changed {
            changed = false;
            for n in 1..n_blocks {
                let mut new = vec![true; n_blocks];
                for p in &preds[n] {
                    for i in 0..n_blocks {
                        new[i] = new[i] && dom[*p][i];
                    }
                }
                new[n] = true;
                if new != dom[n] {
                    dom[n] = new;
                    changed = true;
                }
            }
        }
    }
    let mut loops: Vec<(Vec<String>, Vec<bool>)> = Vec::new();
    for (latch, b) in blocks.iter().enumerate() {
        if b.loop_ids.is_empty() {
            continue;
        }
        for header_name in &b.succs {
            let Some(&header) = index.get(header_name.as_str()) else {
                continue;
            };
            if !dom[latch][header] {
                continue;
            }
            let mut in_loop = vec![false; blocks.len()];
            in_loop[header] = true;
            let mut stack = vec![latch];
            while let Some(n) = stack.pop() {
                if in_loop[n] {
                    continue;
                }
                in_loop[n] = true;
                stack.extend(preds[n].iter().copied());
            }
            loops.push((b.loop_ids.clone(), in_loop));
        }
    }
    let has_div = |set: &Vec<bool>| {
        blocks
            .iter()
            .zip(set)
            .any(|(blk, inside)| *inside && blk.has_const_div)
    };
    let div_sets: Vec<&Vec<bool>> = loops
        .iter()
        .map(|(_, set)| set)
        .filter(|set| has_div(set))
        .collect();
    // Innermost divide loops: no other divide loop strictly inside.
    // The nest is their ancestors and descendants; a sibling loop
    // inside a shared outer loop is neither and stays rolled.
    let subset = |a: &Vec<bool>, b: &Vec<bool>| a.iter().zip(b.iter()).all(|(x, y)| !x || *y);
    let innermost: Vec<&Vec<bool>> = div_sets
        .iter()
        .filter(|set| {
            !div_sets
                .iter()
                .any(|other| subset(other, set) && !subset(set, other))
        })
        .copied()
        .collect();
    let mut ids: Vec<String> = Vec::new();
    for (loop_ids, set) in &loops {
        let in_nest = innermost
            .iter()
            .any(|div_set| subset(div_set, set) || subset(set, div_set));
        if in_nest {
            for id in loop_ids {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    let any = !ids.is_empty();
    (ids, any)
}

/// Remove `llvm.loop.unroll.disable` from the nest of a constant
/// divide, returning the edited text and the functions owning those
/// loops. clang `-O1` stamps the mark on every loop and the
/// unroller honours it unconditionally, so no threshold alone can
/// unroll `-O1` IR. Scoping to the nest keeps sibling loops rolled.
fn strip_unroll_disable(ll_text: &str) -> (String, Vec<String>) {
    strip_unroll_disable_for(ll_text, None)
}

/// `strip_unroll_disable` restricted to the named functions, used to
/// rerun the fold with only the targets that actually shrank.
fn strip_unroll_disable_for(ll_text: &str, only: Option<&[String]>) -> (String, Vec<String>) {
    let lines: Vec<&str> = ll_text.lines().collect();
    let mut disable_ids: Vec<String> = Vec::new();
    for line in &lines {
        let t = line.trim();
        if (t.contains("= !{!\"llvm.loop.unroll.disable\"}")
            || t.contains("= distinct !{!\"llvm.loop.unroll.disable\"}"))
            && t.starts_with('!')
        {
            if let Some(eq) = t.find(" =") {
                disable_ids.push(t[1..eq].to_string());
            }
        }
    }
    if disable_ids.is_empty() {
        return (ll_text.to_string(), Vec::new());
    }
    let mut loop_ids: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for span in function_spans(&lines) {
        if let Some(only) = only {
            if !only.contains(&span.name) {
                continue;
            }
        }
        let (ids, any) = const_div_loops(&lines, &span);
        if any {
            targets.push(span.name.clone());
            for id in ids {
                if !loop_ids.contains(&id) {
                    loop_ids.push(id);
                }
            }
        }
    }
    if loop_ids.is_empty() {
        return (ll_text.to_string(), Vec::new());
    }
    let mut out = String::with_capacity(ll_text.len());
    for line in &lines {
        let t = line.trim_start();
        if !loop_ids.iter().any(|id| t.starts_with(&format!("!{id} ="))) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut owned = line.to_string();
        for id in &disable_ids {
            let tok = format!("!{id}");
            owned = owned
                .replace(&format!(", {tok},"), ",")
                .replace(&format!(", {tok}}}"), "}")
                .replace(&format!(", {tok})"), ")")
                .replace(&format!("{{{tok}, "), "{");
        }
        out.push_str(&owned);
        out.push('\n');
    }
    (out, targets)
}

/// One instruction line inside a function body, debug excluded.
fn is_instruction(t: &str) -> bool {
    !t.starts_with("#dbg")
        && !t.ends_with(':')
        && (t.starts_with('%')
            || t.starts_with("store ")
            || t.starts_with("br ")
            || t.starts_with("call ")
            || t.starts_with("tail call ")
            || t.starts_with("ret ")
            || t.starts_with("switch ")
            || t.starts_with("unreachable"))
}

/// Per-function IR instruction counts. The fold guard compares
/// these: every target must shrink and no other function may grow.
fn function_instruction_counts(ll_text: &str) -> std::collections::HashMap<String, usize> {
    let lines: Vec<&str> = ll_text.lines().collect();
    let mut out = std::collections::HashMap::new();
    for span in function_spans(&lines) {
        let n = lines[span.start + 1..span.end]
            .iter()
            .filter(|l| is_instruction(l.trim()))
            .count();
        out.insert(span.name.clone(), n);
    }
    out
}

/// One `opt` invocation: `input` to `out`, returning the output text.
fn run_opt(
    opt_bin: &Path,
    input: &Path,
    out: &Path,
    passes: &str,
    args: &[String],
) -> Result<String, String> {
    let mut cmd = Command::new(opt_bin);
    cmd.arg("-S").arg(format!("-passes={passes}"));
    for a in args {
        cmd.arg(a);
    }
    cmd.arg(input).args(["-o", out.to_str().unwrap()]);
    let res = cmd.output().map_err(|e| format!("run opt: {e}"))?;
    if !res.status.success() {
        return Err(String::from_utf8_lossy(&res.stderr).into_owned());
    }
    std::fs::read_to_string(out).map_err(|e| format!("read {}: {e}", out.display()))
}

/// Run the curated whole-program cleanup over `merged_path` (the
/// `llvm-link` output), writing the result to `out_path`. Returns the
/// optimized `.ll` text. The profile selects the pipeline, not clang's
/// flags (clang stays at `-O1` under every profile): `-O0` folds
/// whole-program constants only (`PASSES_O0`, control flow untouched, for
/// stepping and bisection); `-O1` runs the base `PASSES` with no
/// cross-function folding; `-Os` adds the single-call-site folds into
/// ordinary callers (today's pipeline exactly); `-O2` additionally folds
/// small single-call-site callees into `main`/ISR roots and leaves code
/// factoring to the driver (which disables it under `-O2`).
pub fn run(
    opt_bin: &Path,
    merged_path: &Path,
    out_path: &Path,
    core: device::Core,
    opt_level: OptLevel,
) -> Result<String, String> {
    let ll_text = std::fs::read_to_string(merged_path)
        .map_err(|e| format!("read {}: {e}", merged_path.display()))?;
    let api = public_api(&ll_text);
    if api.is_empty() {
        // No `main`/ISR found (e.g. a library-only compile that never
        // reaches wholeprog's "exactly one main" check), so internalizing
        // everything would be unsound; skip the stage instead, downstream
        // still gets the plain merged IR.
        return Ok(ll_text);
    }
    // `-O0` folds constants but restructures nothing; `-O1` runs the base
    // list with no folding at all; `-O2` folds small single-call-site
    // callees even into `main`/ISR roots; `-Os` keeps today's folds into
    // ordinary callers only.
    let candidates = match opt_level {
        OptLevel::O0 | OptLevel::O1 => Vec::new(),
        OptLevel::O2 => always_inline_candidates(&ll_text, true),
        OptLevel::Os => always_inline_candidates(&ll_text, false),
    };
    let marked = mark_always_inline(&ll_text, &candidates);
    // Both cores run LSR now (epic-cc#647 fixed the PIC14 alloca-address
    // defect that gated it, epic-cc#645 measured the gains), so the base
    // list is core-independent. `core` stays a parameter: the call site is
    // core-agnostic by design, and a future core-specific deviation has its
    // hook here.
    let _ = core;
    let base = if opt_level == OptLevel::O0 {
        PASSES_O0
    } else {
        PASSES
    };
    let passes = if candidates.is_empty() {
        base.to_string()
    } else {
        format!("{base},always-inline,instcombine,simplifycfg,dce")
    };
    std::fs::write(merged_path, &marked)
        .map_err(|e| format!("write {}: {e}", merged_path.display()))?;
    let api_arg = format!("--internalize-public-api-list={}", api.join(","));
    let base_text = run_opt(opt_bin, merged_path, out_path, &passes, &[api_arg])?;
    if opt_level == OptLevel::O0 {
        return Ok(base_text);
    }
    // Fold attempt (epic-cc#1212): unroll the now-constant search.
    // Gated on the strip actually changing the text, and kept only
    // when a target function shrinks and no function grows. Targets
    // that do not shrink are dropped and the fold reruns without
    // them, so their loops are never unrolled in the shipped text.
    let (stripped, targets) = strip_unroll_disable(&base_text);
    if targets.is_empty() || stripped == base_text {
        return Ok(base_text);
    }
    let fold_in = out_path.with_extension("fold_in.ll");
    let fold_out = out_path.with_extension("fold.ll");
    let threshold = format!("--unroll-threshold={FOLD_UNROLL_THRESHOLD}");
    let attempt = |text: &str| -> Option<String> {
        std::fs::write(&fold_in, text).ok()?;
        run_opt(
            opt_bin,
            &fold_in,
            &fold_out,
            FOLD_PASSES,
            &[threshold.clone()],
        )
        .ok()
    };
    let Some(mut folded) = attempt(&stripped) else {
        return Ok(base_text);
    };
    let base_counts = function_instruction_counts(&base_text);
    let shrunk = |counts: &std::collections::HashMap<String, usize>, name: &str| {
        counts.get(name).copied().unwrap_or(0) < base_counts.get(name).copied().unwrap_or(0)
    };
    let first_counts = function_instruction_counts(&folded);
    let shrinking: Vec<String> = targets
        .iter()
        .filter(|name| shrunk(&first_counts, name))
        .cloned()
        .collect();
    if shrinking.is_empty() {
        return Ok(base_text);
    }
    if shrinking.len() != targets.len() {
        let (reduced, _) = strip_unroll_disable_for(&base_text, Some(&shrinking));
        let Some(rerun) = attempt(&reduced) else {
            return Ok(base_text);
        };
        folded = rerun;
    }
    let fold_counts = function_instruction_counts(&folded);
    let any_target_shrinks = shrinking.iter().any(|name| shrunk(&fold_counts, name));
    let nothing_grows = fold_counts
        .iter()
        .all(|(name, n)| *n <= base_counts.get(name).copied().unwrap_or(0));
    if !any_target_shrinks || !nothing_grows {
        return Ok(base_text);
    }
    std::fs::write(out_path, &folded).map_err(|e| format!("write {}: {e}", out_path.display()))?;
    Ok(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_api_keeps_main_and_isr_only() {
        let ll = "\
define dso_local void @helper() #0 {
}
define dso_local i16 @main() #0 {
}
define dso_local msp430_intrcc void @PIC16_IRQ_Handler() #1 {
}
";
        let mut api = public_api(ll);
        api.sort();
        assert_eq!(
            api,
            vec!["PIC16_IRQ_Handler".to_string(), "main".to_string()]
        );
    }

    #[test]
    fn public_api_keeps_variadic_functions() {
        let ll = "\
define dso_local i16 @main() #0 {
}
define dso_local i16 @printf(ptr nocapture noundef readonly %0, ...) #0 {
}
";
        let mut api = public_api(ll);
        api.sort();
        assert_eq!(api, vec!["main".to_string(), "printf".to_string()]);
    }

    #[test]
    fn public_api_keeps_mutable_globals_not_constants() {
        let ll = "\
define dso_local i16 @main() #0 {
}
@in_a = dso_local global i16 0, align 2
@.str = private unnamed_addr constant [4 x i8] c\"abc\\00\", align 1
";
        let mut api = public_api(ll);
        api.sort();
        assert_eq!(api, vec!["in_a".to_string(), "main".to_string()]);
    }

    #[test]
    fn public_api_empty_without_main() {
        let ll = "define dso_local void @helper() #0 {\n}\n";
        assert!(public_api(ll).is_empty());
    }

    #[test]
    fn public_api_keeps_mangled_cpp_entry() {
        let ll = "\
define dso_local noundef i16 @_Z4mainv() #0 {
}
define dso_local void @helper() #0 {
}
";
        assert_eq!(public_api(ll), vec!["_Z4mainv".to_string()]);
        let lines: Vec<&str> = ll.lines().collect();
        let spans = function_spans(&lines);
        assert!(
            spans
                .iter()
                .find(|s| s.name == "_Z4mainv")
                .expect("span")
                .is_entry
        );
        assert!(
            !spans
                .iter()
                .find(|s| s.name == "helper")
                .expect("span")
                .is_entry
        );
    }

    #[test]
    fn always_inline_finds_a_single_call_site_into_an_ordinary_function() {
        let ll = "\
define dso_local i16 @main() #0 {
  tail call void @driver()
  ret i16 0
}
define internal void @driver() #0 {
  tail call void @helper()
  ret void
}
define internal void @helper() #0 {
  ret void
}
";
        assert_eq!(
            always_inline_candidates(ll, false),
            vec!["helper".to_string()],
            "driver's one caller is main, so driver stays; helper's one caller is driver, an ordinary function, so helper is a safe fold"
        );
    }

    #[test]
    fn always_inline_excludes_multi_call_site_functions() {
        let ll = "\
define internal void @a() #0 {
  tail call void @shared()
  ret void
}
define internal void @b() #0 {
  tail call void @shared()
  ret void
}
define internal void @shared() #0 {
  ret void
}
";
        assert!(always_inline_candidates(ll, false).is_empty());
    }

    #[test]
    fn always_inline_excludes_address_taken_functions() {
        let ll = "\
define internal void @caller() #0 {
  tail call void @cb()
  ret void
}
define internal void @cb() #0 {
  ret void
}
@table = internal global [1 x ptr] [ptr @cb]
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "cb is also stored in a global table, folding its one direct call would leave a dangling indirect path"
        );
    }

    #[test]
    fn always_inline_excludes_functions_marked_noinline_via_attribute_group() {
        let ll = "\
define internal void @caller() #0 {
  tail call void @helper()
  ret void
}
define internal void @helper() #1 {
  ret void
}
attributes #0 = { nounwind }
attributes #1 = { noinline nounwind }
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "helper is noinline, LLVM rejects noinline+alwaysinline on the same function"
        );
    }

    #[test]
    fn aggressive_folds_a_small_single_call_site_callee_into_main() {
        let ll = "\
define dso_local i16 @main() #0 {
  tail call void @driver()
  ret i16 0
}
define internal void @driver() #0 {
  ret void
}
:";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "the default tier never folds into main: its frame never returns"
        );
        assert_eq!(
            always_inline_candidates(ll, true),
            vec!["driver".to_string()],
            "the aggressive tier folds a small one-caller helper into main"
        );
    }

    #[test]
    fn strip_unroll_disable_keeps_sibling_nested_in_a_shared_outer_loop() {
        let ll = "define i16 @main() #0 {\n  br label %ohead\nohead:\n  br label %shead\nshead:\n  %n = udiv i32 5000, %pq\n  br i1 %c, label %shead, label %rhead, !llvm.loop !4\nrhead:\n  %m = mul i32 %a, 3\n  br i1 %d, label %rhead, label %onext, !llvm.loop !7\nonext:\n  br i1 %e, label %ohead, label %exit, !llvm.loop !9\nexit:\n  ret i16 0\n}\n!4 = distinct !{!4, !6}\n!7 = distinct !{!7, !6}\n!9 = distinct !{!9, !6}\n!6 = !{!\"llvm.loop.unroll.disable\"}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert_eq!(targets, vec!["main".to_string()]);
        assert!(
            out.contains("!4 = distinct !{!4}"),
            "search loop unmarked: {out}"
        );
        assert!(
            out.contains("!9 = distinct !{!9}"),
            "outer loop unmarked: {out}"
        );
        assert!(
            out.contains("!7 = distinct !{!7, !6}"),
            "nested sibling keeps its mark: {out}"
        );
    }

    #[test]
    fn aggressive_leaves_a_large_single_call_site_callee_out_of_line() {
        let mut ll = String::from(
            "define dso_local i16 @main() #0 {\n  tail call void @big()\n  ret i16 0\n}\ndefine internal void @big() #0 {\n",
        );
        for _ in 0..(MAX_ENTRY_INLINE_LINES + 10) {
            ll.push_str("  %x = add i16 0, 1\n");
        }
        ll.push_str("  ret void\n}\n");
        assert!(
            always_inline_candidates(&ll, true).is_empty(),
            "folding a large body into main would pin its whole frame in RAM permanently"
        );
    }

    #[test]
    fn mark_always_inline_inserts_before_metadata_attachments() {
        let ll = "define internal void @helper(i16 %0) local_unnamed_addr #3 !dbg !9 {\n}\n";
        let marked = mark_always_inline(ll, &["helper".to_string()]);
        assert_eq!(
            marked,
            "define internal void @helper(i16 %0) local_unnamed_addr #3 alwaysinline !dbg !9 {\n}\n"
        );
    }

    #[test]
    fn mark_always_inline_inserts_before_the_brace_when_no_metadata() {
        let ll = "define internal void @helper(i16 %0) local_unnamed_addr #3 {\n}\n";
        let marked = mark_always_inline(ll, &["helper".to_string()]);
        assert_eq!(
            marked,
            "define internal void @helper(i16 %0) local_unnamed_addr #3 alwaysinline {\n}\n"
        );
    }

    #[test]
    fn strip_unroll_disable_covers_loops_nested_in_a_const_div_loop() {
        let ll = "define i16 @main() #0 {\n  br label %shead\nshead:\n  %n = udiv i32 5000, %pq\n  br label %ihead\nihead:\n  %m = mul i32 %a, 3\n  br i1 %c, label %ihead, label %snext, !llvm.loop !7\nsnext:\n  br i1 %d, label %shead, label %exit, !llvm.loop !4\nexit:\n  ret i16 0\n}\n!4 = distinct !{!4, !6}\n!7 = distinct !{!7, !6}\n!6 = !{!\"llvm.loop.unroll.disable\"}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert_eq!(targets, vec!["main".to_string()]);
        assert!(
            out.contains("!4 = distinct !{!4}"),
            "outer loop unmarked: {out}"
        );
        assert!(
            out.contains("!7 = distinct !{!7}"),
            "nested loop unmarked: {out}"
        );
    }
    #[test]
    fn strip_unroll_disable_removes_mark_from_const_div_loops() {
        let ll = "define i16 @main() #0 {\n  br label %head\nhead:\n  %n = udiv i32 5000, %pq\n  br i1 %c, label %head, label %exit, !llvm.loop !4\nexit:\n  ret i16 0\n}\n!4 = distinct !{!4, !5, !6}\n!5 = !{!\"llvm.loop.mustprogress\"}\n!6 = !{!\"llvm.loop.unroll.disable\"}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert_eq!(targets, vec!["main".to_string()]);
        assert!(
            out.contains("!4 = distinct !{!4, !5}"),
            "mark removed from loop node: {out}"
        );
        assert!(out.contains("!llvm.loop !4"), "loop attachment kept: {out}");
    }

    #[test]
    fn strip_unroll_disable_keeps_sibling_runtime_loop_marks() {
        let ll = "define i16 @main() #0 {\n  br label %shead\nshead:\n  %n = udiv i32 5000, %pq\n  br i1 %c, label %shead, label %rhead, !llvm.loop !4\nrhead:\n  %m = mul i32 %a, 3\n  br i1 %d, label %rhead, label %exit, !llvm.loop !7\nexit:\n  ret i16 0\n}\n!4 = distinct !{!4, !6}\n!7 = distinct !{!7, !6}\n!6 = !{!\"llvm.loop.unroll.disable\"}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert_eq!(targets, vec!["main".to_string()]);
        assert!(
            out.contains("!4 = distinct !{!4}"),
            "search loop unmarked: {out}"
        );
        assert!(
            out.contains("!7 = distinct !{!7, !6}"),
            "runtime loop keeps its mark: {out}"
        );
    }

    #[test]
    fn strip_unroll_disable_keeps_runtime_loop_marks() {
        let ll = "define i16 @main() #0 {\n  br label %head\nhead:\n  br i1 %c, label %head, label %exit, !llvm.loop !4\nexit:\n  ret i16 0\n}\n!4 = distinct !{!4, !5, !6}\n!5 = !{!\"llvm.loop.mustprogress\"}\n!6 = !{!\"llvm.loop.unroll.disable\"}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert!(targets.is_empty());
        assert_eq!(out, ll);
    }

    #[test]
    fn strip_unroll_disable_without_node_is_identity() {
        let ll = "define i16 @main() #0 {\n  ret i16 0\n}\n";
        let (out, targets) = strip_unroll_disable(ll);
        assert!(targets.is_empty());
        assert_eq!(out, ll);
    }

    #[test]
    fn function_instruction_counts_skips_debug_and_metadata() {
        let ll = "define i16 @main() #0 !dbg !9 {\n  %a = add i16 1, 2\n  #dbg_value(i16 %a, !1, !DIExpression(), !2)\n  store i16 %a, ptr @g\n  ret i16 %a\n}\n!9 = !{}\n";
        let counts = function_instruction_counts(ll);
        assert_eq!(counts.get("main"), Some(&3));
    }
}
