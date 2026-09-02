//! Whole-program IR cleanup: runs LLVM `opt` over the `llvm-link`-merged
//! module before it reaches `irparse`.
//!
//! Each `.c` file is compiled by clang **alone**: clang never sees that
//! `epic_tick_init(fosc_hz)`'s only caller always passes the literal
//! `FOSC_HZ`, so the whole nested-loop period search in the file-local
//! helper it calls survives untouched into every TU's `.ll`. But by the
//! time `llvm-link` has merged every TU, the whole call graph — and every
//! cross-TU constant argument — is visible in one module. Nothing after
//! `llvm-link` ever re-derives that, so a whole-program compiler that
//! skips this step is leaving exactly the constant-folding a linking,
//! whole-program compiler exists to do on the table (epic-cc#193).
//!
//! The pass list is deliberately narrow, chosen to preserve the property
//! the overlay allocator (`alloc`) depends on: a function's locals are
//! only live between its call and return, so two functions that are never
//! simultaneously live can share RAM. A generic `-O2` inlines aggressively
//! enough to merge callee locals permanently into `main`'s frame (`main`
//! never returns, so nothing it ever contains can be reclaimed) — measured
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
//!
//! Measured on the epic-encoder full example (epic-cc#193, epic-hal's
//! `epic-encoder` module on the 16F877A — gpio+timer0+timer2+ssp+usart+
//! irq+wdt+dispatch+tick+encoder+serial+the example TU): 13281 -> 7519
//! words (-43%), RAM 358 -> 350/368 bytes (still fits). That takes the
//! full example from 62% over the 877A's real 8192-word flash budget to
//! 7519/8192 (91.8%) — it links. XC8 builds the same source combination
//! at 5356/8192 (65.4%), so epic-cc is now within 1.4x of XC8 instead of
//! 2.5x.

use std::path::Path;
use std::process::Command;

/// The curated, RAM-safe pass list (see module docs for why each pass is
/// here and none of them inline across a call boundary).
const PASSES: &str = "internalize,ipsccp,instcombine,simplifycfg,dce";

/// Symbols `internalize` must never touch:
///
/// - `main` (the whole program's one entry point) and every
///   `msp430_intrcc` interrupt handler (the vector table's entry points —
///   `irparse` identifies them the same way, by that calling-convention
///   token on the `define` line).
/// - Every **variadic** function (a `(...)` parameter list). Load-bearing,
///   not caution for its own sake: measured on epic-cc#131's `printf`
///   acceptance fixture, internalizing a single-call-site variadic
///   function (`printf`, called once with a literal format string) lets
///   `ipsccp` replace every use of its named format-string parameter with
///   that literal — sound as a pure value substitution, but our
///   `llvm.va_start` lowering locates the first vararg relative to that
///   parameter's own frame slot, and a parameter with no remaining SSA
///   uses doesn't reliably get one allocated. The result silently
///   corrupts the vararg walk (one format byte read as 0) with no panic
///   to catch it. Named parameters immediately before `...` are exactly
///   the ones `va_start` depends on this way, so the whole function stays
///   external — external-linkage arguments are never specialized by
///   `ipsccp`. The cost is a constant format string one call deep not
///   getting folded into `printf`'s callee; no measured example needed
///   that.
/// - Every **mutable global variable** (LLVM `global`, not `constant`).
///   Also load-bearing: measured on epic-cc#133's pid-clamp fixture,
///   internalizing a plain (non-`const`, non-`volatile`) global that
///   nothing in the compiled program ever stores to lets `ipsccp` read it
///   as permanently equal to its zero-initializer — correct for a truly
///   closed program, but epic-cc's own e2e harness (and real embedded
///   code with a memory-mapped input) writes such globals from outside
///   the compiled image, a channel no IR-level analysis can see. A
///   `constant` global (string literals, `irq_table`-style const data)
///   has no such hazard — nothing ever writes it by construction — so
///   those stay eligible and are exactly what the `Base::Global`-as-value
///   isel fix (epic-cc#193) exists to let flow through `ipsccp` safely.
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
            // else — aliases, ifuncs) fall through and stay eligible.
            let name_end = line[1..]
                .find(|c: char| c == '=' || c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(line.len());
            api.push(line[1..name_end].to_string());
        }
    }
    api
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
        // reaches wholeprog's "exactly one main" check) — internalizing
        // everything would be unsound, so skip the stage; downstream
        // still gets the plain merged IR.
        return Ok(ll_text);
    }
    let mut cmd = Command::new(opt_bin);
    cmd.arg("-S")
        .arg(format!("--internalize-public-api-list={}", api.join(",")))
        .arg(format!("-passes={PASSES}"))
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
}
