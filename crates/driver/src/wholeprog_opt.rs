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
//! never returns, so nothing it ever contains can be reclaimed), measured
//! to blow the 877A's 368-byte budget on this ticket's example. None of
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
//!     `mark_always_inline`, epic-cc#205): folds only the functions with
//!     exactly one direct call site, into an ordinary (non-`main`,
//!     non-ISR) caller, never referenced any other way. That caller
//!     already reclaims its frame on return, so the callee's locals do
//!     too, same as before the fold; this is free on every axis, no
//!     RAM/flash trade, unlike inlining into `main`/an ISR root (that
//!     shape trades flash for permanent RAM and needs the future `-O2`
//!     "aggressive" tier's explicit budget check, epic-cc#204). Marks
//!     candidates with `alwaysinline` textually, so LLVM's `always-inline`
//!     pass folds exactly this set and nothing else, no cost-model
//!     guessing. Skipped entirely (falls back to the base `PASSES` list)
//!     when no candidates are found.
//!
//! Measured on the epic-encoder full example (epic-cc#193, epic-hal's
//! `epic-encoder` module on the 16F877A: gpio+timer0+timer2+ssp+usart+
//! irq+wdt+dispatch+tick+encoder+serial+the example TU): 13281 -> 7519
//! words (-43%), RAM 358 -> 350/368 bytes (still fits). That takes the
//! full example from 62% over the 877A's real 8192-word flash budget to
//! 7519/8192 (91.8%), it links. XC8 builds the same source combination
//! at 5356/8192 (65.4%), so epic-cc is now within 1.4x of XC8 instead of
//! 2.5x.
//!
//! Two further, independent fixes stack on top of that baseline
//! (epic-cc#205/#206): consolidating `printf`'s per-call-site literal
//! staging buffer into one shared buffer (apojomovsky/epic-hal#123/#124)
//! took the full example to 7327 words / 332 bytes RAM; adding the
//! always-inline pass above on top of that takes it to 7240 words / 329
//! bytes RAM. The always-inline pass alone (no buffer fix in play) also
//! measurably helps the plain `hal-pic16-blink` example: 690 -> 652 words,
//! 59 -> 47 bytes RAM. Note this does **not** close epic-cc#205's
//! deepest-call-chain finding by itself: that chain's critical hop folds
//! into `main`, which `always_inline_candidates` deliberately excludes by
//! design (see above); closing that hop is the deferred `-O2` tier's job.

use std::path::Path;
use std::process::Command;

/// The curated, RAM-safe pass list (see module docs for why each pass is
/// here and none of them inline across a call boundary).
const PASSES: &str = "internalize,ipsccp,instcombine,simplifycfg,dce";

/// Symbols `internalize` must never touch:
///
/// - `main` (the whole program's one entry point) and every
///   `msp430_intrcc` interrupt handler (the vector table's entry points,
///   `irparse` identifies them the same way, by that calling-convention
///   token on the `define` line).
/// - Every **variadic** function (a `(...)` parameter list). Load-bearing,
///   not caution for its own sake: measured on epic-cc#131's `printf`
///   acceptance fixture, internalizing a single-call-site variadic
///   function lets `ipsccp` replace every use of its named format-string
///   parameter with the caller's literal, sound as a pure value
///   substitution, but our `llvm.va_start` lowering locates the first
///   vararg relative to that parameter's own frame slot, and a parameter
///   with no remaining SSA uses doesn't reliably get one allocated. The
///   vararg walk silently corrupts (one format byte read as 0), no panic.
///   So the whole function stays external, since external-linkage
///   arguments are never specialized by `ipsccp`.
/// - Every **mutable global variable** (LLVM `global`, not `constant`).
///   Also load-bearing: measured on epic-cc#133's pid-clamp fixture,
///   internalizing a plain (non-`const`, non-`volatile`) global that
///   nothing in the compiled program ever stores to lets `ipsccp` read it
///   as permanently equal to its zero-initializer. Correct for a truly
///   closed program, but epic-cc's own e2e harness (and real embedded
///   code with a memory-mapped input) writes such globals from outside
///   the compiled image, a channel no IR-level analysis can see. A
///   `constant` global has no such hazard, nothing ever writes it by
///   construction, so those stay eligible.
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
            if name == "main" || line.split_whitespace().any(|t| t == "msp430_intrcc") || variadic {
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

/// One `define`d function's name, entry-point status (`main` or a
/// `msp430_intrcc` ISR), and line range `[start, end]` (the `define`
/// line through its closing `}`, which clang always emits alone on its
/// own unindented line, never sharing a line with a struct-literal `}`).
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
                    name == "main" || line.split_whitespace().any(|t| t == "msp430_intrcc");
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
/// shape is the `-O2` "aggressive" tier's job, not this one, see
/// apojomovsky/epic-cc#204).
fn always_inline_candidates(ll_text: &str) -> Vec<String> {
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
            if caller == usize::MAX || funcs[caller].is_entry {
                return None;
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
                    // before the brace): verified against a real
                    // clang-emitted `!dbg`-carrying line, `opt` otherwise
                    // errors "expected '{' in function body".
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
/// optimized `.ll` text.
pub fn run(opt_bin: &Path, merged_path: &Path, out_path: &Path) -> Result<String, String> {
    let ll_text = std::fs::read_to_string(merged_path)
        .map_err(|e| format!("read {}: {e}", merged_path.display()))?;
    let api = public_api(&ll_text);
    if api.is_empty() {
        // No `main`/ISR found yet (e.g. a library-only compile that never
        // reaches wholeprog's "exactly one main" check), so internalizing
        // everything would be unsound; skip the stage instead, downstream
        // still gets the plain merged IR.
        return Ok(ll_text);
    }
    let candidates = always_inline_candidates(&ll_text);
    let marked = mark_always_inline(&ll_text, &candidates);
    let passes = if candidates.is_empty() {
        PASSES.to_string()
    } else {
        format!("{PASSES},always-inline,instcombine,simplifycfg,dce")
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
            always_inline_candidates(ll),
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
        assert!(always_inline_candidates(ll).is_empty());
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
            always_inline_candidates(ll).is_empty(),
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
            always_inline_candidates(ll).is_empty(),
            "helper is noinline, LLVM rejects noinline+alwaysinline on the same function"
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
