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
//!   - The same marking also folds a multi-call-site function when every
//!     direct call site passes a literal at one argument position, the
//!     literals differ across sites, and that argument indexes a
//!     constant table in the body (epic-cc#1214). `ipsccp` keeps the
//!     shared body for that shape, so the table lookup survives at
//!     every site; a constant index into a constant table folds to
//!     literal fields, which is what makes the duplicated body
//!     collapse. Site-count and body-size caps bound the duplication,
//!     and entry callers stay excluded, so the folded locals still
//!     reclaim when the caller returns.
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
/// Caps for the multi-call-site constant-argument fold: the body is
/// duplicated once per site before the post-inline cleanup folds it, so
/// both bounds price the worst case where that fold does not fire.
const MAX_CONST_ARG_INLINE_SITES: usize = 8;
const MAX_CONST_ARG_INLINE_LINES: usize = 64;

/// Split a parenthesised list's contents at top-level commas. Nested
/// parens, brackets and braces (aggregate types, constant expressions)
/// and quoted strings keep their commas.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        if in_str {
            if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

/// The argument texts of the list opening at `open` in `line`, or `None`
/// when the list never closes on this line.
fn list_arguments(line: &str, open: usize) -> Option<Vec<String>> {
    let inner_start = open + 1;
    let mut depth = 0i32;
    let mut in_str = false;
    for (i, c) in line[inner_start..].char_indices() {
        if in_str {
            if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => {
                let inner = &line[inner_start..inner_start + i];
                return Some(if inner.trim().is_empty() {
                    Vec::new()
                } else {
                    split_top_level(inner)
                });
            }
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The literal an argument text passes, when it passes one this fold
/// keys on: an integer or boolean token in value position.
fn literal_token(arg: &str) -> Option<&str> {
    let tok = arg.split_whitespace().last()?;
    if tok == "true" || tok == "false" || tok.parse::<i64>().is_ok() {
        Some(tok)
    } else {
        None
    }
}

/// Parameter positions that are a literal at every call site with at
/// least two distinct values across sites. A single shared value is
/// `ipsccp`'s case and keeps the out-of-line body; a runtime value at
/// any site means the folded body would still need the general path.
fn varying_constant_positions(params: &[String], sites: &[Vec<String>]) -> Vec<usize> {
    if params.iter().any(|p| p.trim() == "...") {
        return Vec::new();
    }
    (0..params.len())
        .filter(|&pos| {
            let mut values: std::collections::HashSet<&str> = Default::default();
            sites.iter().all(|args| {
                args.len() == params.len()
                    && literal_token(&args[pos]).is_some_and(|v| {
                        values.insert(v);
                        true
                    })
            }) && values.len() >= 2
        })
        .collect()
}

/// Whether `line` mentions the SSA value `tok` as a whole token.
fn mentions_value(line: &str, tok: &str) -> bool {
    line.match_indices(tok).any(|(i, _)| {
        let after = line[i + tok.len()..].chars().next();
        after.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '$')))
    })
}

/// Whether one `getelementptr` line has a constant global as its base
/// operand and the parameter in its index operands. Merely mentioning
/// both on one line is not enough: a fixed-slot table address passed
/// alongside the parameter indexes nothing.
fn gep_indexes_param(
    line: &str,
    tok: &str,
    const_globals: &std::collections::HashSet<&str>,
) -> bool {
    let Some(at) = line.find("getelementptr") else {
        return false;
    };
    let rest = &line[at + "getelementptr".len()..];
    let operands = match rest.find('(') {
        // Constant-expression form: operands sit inside the parens.
        Some(open) => {
            let mut depth = 0i32;
            let mut close = None;
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else { return false };
            &rest[open + 1..close]
        }
        None => rest,
    };
    let segs = split_top_level(operands);
    segs.len() >= 3
        && const_globals
            .iter()
            .any(|g| mentions_value(&segs[1], &format!("@{g}")))
        && segs[2..].iter().any(|s| mentions_value(s, tok))
}

/// Whether the parameter at `pos` indexes a constant global in the
/// body. That shape folds to literal field values once the argument
/// is a literal, so the duplicated body collapses. A constant that
/// only bounds a loop or picks a branch has no such guarantee and
/// keeps the shared body (the menu demo's serial writer regressed on
/// exactly that shape).
fn indexes_constant_table(
    lines: &[&str],
    f: &FuncSpan,
    params: &[String],
    pos: usize,
    const_globals: &std::collections::HashSet<&str>,
) -> bool {
    let Some(tok) = params[pos].split_whitespace().last() else {
        return false;
    };
    if !tok.starts_with('%') {
        return false;
    }
    lines[f.start..=f.end]
        .iter()
        .any(|line| gep_indexes_param(line, tok, const_globals))
}
fn always_inline_candidates(ll_text: &str, aggressive: bool) -> Vec<String> {
    let lines: Vec<&str> = ll_text.lines().collect();
    let funcs = function_spans(&lines);
    let defined: std::collections::HashSet<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    let const_globals: std::collections::HashSet<&str> = lines
        .iter()
        .filter(|l| l.starts_with('@') && l.split_whitespace().any(|t| t == "constant"))
        .filter_map(|l| {
            let end = l[1..]
                .find(|c: char| c == '=' || c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(l.len());
            Some(&l[1..end])
        })
        .collect();
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
    let mut site_args: std::collections::HashMap<&str, Vec<Vec<String>>> = Default::default();
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
                    let open = name_end + (line[name_end..].len() - after.len());
                    site_args
                        .entry(name)
                        .or_default()
                        .push(list_arguments(line, open).unwrap_or_default());
                } else {
                    address_taken.insert(name);
                }
            }
            let advance = (name_end - offset).max(pos + 1);
            rest = &line[offset + advance..];
            offset += advance;
        }
    }

    let mut singles: Vec<String> = Vec::new();
    let mut multis: Vec<String> = Vec::new();
    for (fi, f) in funcs.iter().enumerate() {
        if f.is_entry
            || address_taken.contains(f.name.as_str())
            || noinline.contains(f.name.as_str())
        {
            continue;
        }
        let Some(sites) = call_sites.get(f.name.as_str()) else {
            continue;
        };
        if sites.len() == 1 {
            let caller = sites[0];
            if caller == usize::MAX {
                continue;
            }
            if funcs[caller].is_entry
                && (!aggressive || f.end - f.start + 1 > MAX_ENTRY_INLINE_LINES)
            {
                continue;
            }
            singles.push(f.name.clone());
            continue;
        }
        if sites.len() > MAX_CONST_ARG_INLINE_SITES
            || f.end - f.start + 1 > MAX_CONST_ARG_INLINE_LINES
        {
            continue;
        }
        if sites
            .iter()
            .any(|&caller| caller == usize::MAX || caller == fi || funcs[caller].is_entry)
        {
            continue;
        }
        let define = lines[f.start];
        let Some(params) = define.find('(').and_then(|o| list_arguments(define, o)) else {
            continue;
        };
        let Some(args) = site_args.get(f.name.as_str()) else {
            continue;
        };
        if varying_constant_positions(&params, args)
            .iter()
            .any(|&pos| indexes_constant_table(&lines, f, &params, pos, &const_globals))
        {
            multis.push(f.name.clone());
        }
    }

    // A multi-site body that calls a marked function, or is called by
    // another multi-site body, duplicates past its own caps when the
    // marked callees expand inside each copy. One reached from an
    // entry through a chain of marked functions lands in the entry
    // frame the direct-caller check above meant to keep it out of.
    // Drop those to a fixpoint: unmarking one can clear another.
    let index_of: std::collections::HashMap<&str, usize> = funcs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();
    let callees_of = |name: &str| -> Vec<String> {
        let Some(&idx) = index_of.get(name) else {
            return Vec::new();
        };
        call_sites
            .iter()
            .filter(|(_, callers)| callers.contains(&idx))
            .map(|(callee, _)| callee.to_string())
            .collect()
    };
    loop {
        let marked: std::collections::HashSet<&str> = singles
            .iter()
            .chain(multis.iter())
            .map(String::as_str)
            .collect();
        let multi_set: std::collections::HashSet<&str> =
            multis.iter().map(String::as_str).collect();
        // Candidates an entry caller pulls in, then candidates those
        // pull in, in turn: the aggressive tier folds the first kind
        // into the entry, and the rest follow it there.
        let mut entry_bound: std::collections::HashSet<&str> = Default::default();
        loop {
            let mut grew = false;
            for name in &marked {
                if entry_bound.contains(name) {
                    continue;
                }
                let pulled = call_sites.get(name).is_some_and(|callers| {
                    callers.iter().any(|&c| {
                        c != usize::MAX
                            && (funcs[c].is_entry || entry_bound.contains(funcs[c].name.as_str()))
                    })
                });
                if pulled {
                    entry_bound.insert(name);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let violators: std::collections::HashSet<String> = multis
            .iter()
            .filter(|m| {
                entry_bound.contains(m.as_str())
                    || callees_of(m).iter().any(|c| marked.contains(c.as_str()))
                    || call_sites.get(m.as_str()).is_some_and(|callers| {
                        callers
                            .iter()
                            .any(|&c| c != usize::MAX && multi_set.contains(funcs[c].name.as_str()))
                    })
            })
            .cloned()
            .collect();
        if violators.is_empty() {
            break;
        }
        multis.retain(|m| !violators.contains(m));
    }

    let keep: std::collections::HashSet<&str> = singles
        .iter()
        .chain(multis.iter())
        .map(String::as_str)
        .collect();
    funcs
        .iter()
        .filter(|f| keep.contains(f.name.as_str()))
        .map(|f| f.name.clone())
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

/// Run the curated whole-program cleanup over `merged_path` (the
/// `llvm-link` output), writing the result to `out_path`. Returns the
/// optimized `.ll` text. The profile selects the pipeline, not clang's
/// flags (clang stays at `-O1` under every profile): `-O0` folds
/// whole-program constants only (`PASSES_O0`, control flow untouched, for
/// stepping and bisection); `-O1` runs the base `PASSES` with no
/// cross-function folding; `-Os` adds the single-call-site and
/// constant-argument folds into ordinary callers (today's pipeline
/// exactly); `-O2` additionally folds small single-call-site callees
/// into `main`/ISR roots and leaves code factoring to the driver
/// (which disables it under `-O2`).
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
    let mut cmd = Command::new(opt_bin);
    cmd.arg("-S")
        .arg(format!("--internalize-public-api-list={}", api.join(",")))
        .arg(format!("-passes={passes}"))
        .arg(merged_path)
        .args(["-o", out_path.to_str().unwrap()]);
    let out = cmd.output().map_err(|e| format!("run opt: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    std::fs::read_to_string(out_path).map_err(|e| format!("read {}: {e}", out_path.display()))
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
    fn always_inline_finds_a_multi_call_site_varying_constant_arg() {
        let ll = "\
@table = internal constant [16 x i8] zeroinitializer
define internal void @a() #0 {
  tail call void @shared(i16 noundef 4)
  ret void
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0
  %2 = load i8, ptr %1
  ret void
}
";
        assert_eq!(
            always_inline_candidates(ll, false),
            vec!["shared".to_string()],
            "every site passes a literal and the literals differ, so each caller folds its own copy"
        );
    }

    #[test]
    fn always_inline_leaves_a_constant_merely_mentioned_on_a_gep_line() {
        let ll = "\
@table = internal constant [16 x i8] zeroinitializer
define internal void @a() #0 {
  tail call void @shared(i16 noundef 4)
  ret void
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 0
  %2 = load i8, ptr %1
  %3 = add i16 %0, 1
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "the table address is fixed and the parameter only rides along, nothing folds"
        );
    }

    #[test]
    fn always_inline_leaves_a_multi_call_site_chain() {
        let ll = "\
@table = internal constant [16 x i8] zeroinitializer
define internal void @a() #0 {
  tail call void @outer(i16 noundef 4)
  ret void
}
define internal void @b() #0 {
  tail call void @outer(i16 noundef 9)
  tail call void @inner(i16 noundef 3)
  ret void
}
define internal void @c() #0 {
  tail call void @inner(i16 noundef 5)
  ret void
}
define internal void @outer(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0
  %2 = load i8, ptr %1
  tail call void @inner(i16 noundef 3)
  ret void
}
define internal void @inner(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0
  %2 = load i8, ptr %1
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "marked copies inside marked copies duplicate past either body's own caps"
        );
    }

    #[test]
    fn always_inline_leaves_a_multi_call_site_behind_an_entry_fold() {
        let ll = "\
@table = internal constant [16 x i8] zeroinitializer
define dso_local i16 @main() #0 {
  tail call void @mid()
  ret i16 0
}
define internal void @mid() #0 {
  tail call void @shared(i16 noundef 4)
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0
  %2 = load i8, ptr %1
  ret void
}
";
        assert_eq!(
            always_inline_candidates(ll, false),
            vec!["shared".to_string()],
            "at the default tier mid stays out of line, so shared folds into an ordinary frame"
        );
        assert_eq!(
            always_inline_candidates(ll, true),
            vec!["mid".to_string()],
            "at the aggressive tier mid folds into main, so shared must not follow it there"
        );
    }

    #[test]
    fn always_inline_leaves_a_multi_call_site_shared_constant_arg() {
        let ll = "\
define internal void @a() #0 {
  tail call void @shared(i16 noundef 4)
  ret void
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 4)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "one shared literal is ipsccp's case, the out-of-line body stays"
        );
    }

    #[test]
    fn always_inline_leaves_a_multi_call_site_with_a_runtime_arg() {
        let ll = "\
define internal void @a(i16 noundef %0) #0 {
  tail call void @shared(i16 noundef %0)
  ret void
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "a runtime value at one site means the general body is still needed"
        );
    }

    #[test]
    fn always_inline_leaves_a_constant_arg_callee_called_from_main() {
        let ll = "\
@table = internal constant [16 x i8] zeroinitializer
define dso_local i16 @main() #0 {
  tail call void @shared(i16 noundef 4)
  ret i16 0
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0
  %2 = load i8, ptr %1
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "a main caller never returns, so its folded locals would stay live for good"
        );
        assert!(
            always_inline_candidates(ll, true).is_empty(),
            "the aggressive tier only widens the single-call-site fold, not this one"
        );
    }

    #[test]
    fn always_inline_leaves_a_varying_constant_that_indexes_no_table() {
        let ll = "\
define internal void @a() #0 {
  tail call void @shared(i16 noundef 4)
  ret void
}
define internal void @b() #0 {
  tail call void @shared(i16 noundef 9)
  ret void
}
define internal void @shared(i16 noundef %0) #0 {
  %1 = icmp eq i16 %0, 0
  ret void
}
";
        assert!(
            always_inline_candidates(ll, false).is_empty(),
            "a constant that only picks a branch or bounds a loop may not shrink the body, so it stays shared"
        );
    }

    #[test]
    fn always_inline_caps_constant_arg_site_count_and_body_size() {
        let mut many = String::from("@table = internal constant [16 x i8] zeroinitializer\n");
        for i in 0..(MAX_CONST_ARG_INLINE_SITES + 1) {
            many.push_str(&format!(
                "define internal void @c{i}() #0 {{\n  tail call void @shared(i16 noundef {i})\n  ret void\n}}\n"
            ));
        }
        many.push_str("define internal void @shared(i16 noundef %0) #0 {\n  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0\n  ret void\n}\n");
        assert!(
            always_inline_candidates(&many, false).is_empty(),
            "past the site cap the duplicated bodies price worse than the shared one"
        );

        let mut big = String::from(
            "@table = internal constant [16 x i8] zeroinitializer\ndefine internal void @a() #0 {\n  tail call void @shared(i16 noundef 4)\n  ret void\n}\ndefine internal void @b() #0 {\n  tail call void @shared(i16 noundef 9)\n  ret void\n}\ndefine internal void @shared(i16 noundef %0) #0 {\n  %1 = getelementptr inbounds [16 x i8], ptr @table, i16 0, i16 %0\n",
        );
        for _ in 0..MAX_CONST_ARG_INLINE_LINES {
            big.push_str("  %x = add i16 %0, 1\n");
        }
        big.push_str("  ret void\n}\n");
        assert!(
            always_inline_candidates(&big, false).is_empty(),
            "past the body cap the duplicated bodies price worse than the shared one"
        );
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
}
