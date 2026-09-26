//! How failures read: internal errors vs user errors.
//!
//! Panics stay the error surface. This module only changes their
//! presentation: bugs print as internal compiler errors with a report
//! pointer, deliberate input errors print as errors. Backtraces follow
//! `RUST_BACKTRACE`, matching the default hook's contract.

/// Report pointer for internal errors.
pub const ISSUE_URL: &str = "https://github.com/apojomovsky/epic-cc/issues";

/// Prefix every deliberate user-facing panic carries. The hook prints such
/// payloads verbatim instead of wrapping them as internal errors.
pub const USER_PREFIX: &str = "epic-cc: error:";

/// Payloads that are deliberate user errors rather than bugs. Each entry is
/// a message shape the pipeline emits for bad input, never for an
/// invariant violation: recursion and depth (callgraph emits only those),
/// missing or dangling entry points (wholeprog), programs that do not fit
/// RAM, pages, or flash (alloc, isel, asm), and unsupported constructs
/// (`not supported`, the marker the fuzz classifier also keys on). A new
/// user-error panic earns an entry here plus a unit test below; anything
/// unlisted stays an internal error.
const USER_PATTERNS: &[&str] = &[
    "epic-cc: error:",
    "callgraph:",
    "wholeprog: expected exactly one",
    "wholeprog: undefined symbols",
    "GPR demand exceeds",
    "needs a bank past",
    "no arrangement of",
    "post-banking page-fit failure",
    "isel: function @",
    "crosses the 256-word ceiling",
    "slot offset ",
    "baseline const index ",
    "exceeds device flash",
    "not supported",
    "unsupported",
];

/// Whether a panic payload is a deliberate user error, not a bug.
pub fn is_user_error(msg: &str) -> bool {
    let text = msg.trim_start();
    if has_error_at(text) {
        return true;
    }
    // A located payload (`file:line:col: ...`) classifies on the message
    // past the location: callgraph reports recursion at its call site.
    let inner;
    let inner_ref = match split_loc(text) {
        Some((_, rest)) => {
            inner = rest;
            &inner
        }
        None => text,
    };
    USER_PATTERNS.iter().any(|p| inner_ref.contains(p))
}

/// Split a leading `<file>:<line>:<col>:` location off `msg`.
fn split_loc(msg: &str) -> Option<(String, String)> {
    let mut parts = msg.splitn(4, ':');
    let (Some(file), Some(line), Some(col), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if file.is_empty() || line.trim().parse::<u32>().is_err() || col.trim().parse::<u32>().is_err()
    {
        return None;
    }
    Some((
        format!("{}:{}:{}", file, line.trim(), col.trim()),
        rest.trim_start().to_string(),
    ))
}

/// Whether `msg` already reads `<file>:<line>:<col>: error: ...`.
fn has_error_at(msg: &str) -> bool {
    match split_loc(msg) {
        Some((_, rest)) => rest.starts_with("error:"),
        None => false,
    }
}

/// Pull the panic payload out as text.
fn payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    info.to_string()
}
/// Backtraces print only when `RUST_BACKTRACE` asks for one.
fn backtrace_on() -> bool {
    std::env::var("RUST_BACKTRACE").is_ok_and(|v| v != "0")
}

/// The `RUST_BACKTRACE` footer: a captured backtrace when asked, else the
/// hint that enables one.
fn backtrace_footer() -> String {
    if backtrace_on() {
        format!("{}", std::backtrace::Backtrace::capture())
    } else {
        "note: run with RUST_BACKTRACE=1 for a backtrace.".to_string()
    }
}

/// The full internal-error block for `msg`, without the trailing backtrace
/// footer. Split out so tests can pin the presentation without panicking.
pub fn format_internal_error(msg: &str, version: &str, at: Option<&str>) -> String {
    let mut out = format!("epic-cc: internal compiler error: {msg}\n");
    if let Some(loc) = at {
        out.push_str(&format!("at {loc}\n"));
    }
    out.push_str(&format!("version: epic-cc {version}\n"));
    out.push_str(&format!(
        "Please report it at {ISSUE_URL} with the failing source, \
         the output of `epic-cc --version`, and the command you ran.\n"
    ));
    out
}

/// Install the presentation hook. Call once at startup.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = payload(info);
        if let Some(line) = user_error_line(&msg) {
            eprintln!("{}", line.text);
            // Provenance for heuristically recognized backend panics: the
            // fuzz classifier keys compiler-side failures on `panicked
            // at` or `epic-cc:`, and the line preserves that without a
            // backtrace. Explicitly marked errors stay one line.
            if line.heuristic {
                if let Some(loc) = info.location() {
                    eprintln!("panicked at {loc}");
                }
            }
            return;
        }
        let at = info.location().map(|l| l.to_string());
        eprint!(
            "{}",
            format_internal_error(&msg, env!("EPIC_CC_STAMP"), at.as_deref())
        );
        eprintln!("{}", backtrace_footer());
    }));
}

/// A user-error payload rendered as its diagnostic line, plus whether the
/// recognition was heuristic (backend pattern) rather than explicit (the
/// `epic-cc: error:` mark or a `file:line:col: error:` shape).
struct UserLine {
    text: String,
    heuristic: bool,
}

fn user_error_line(msg: &str) -> Option<UserLine> {
    let text = msg.trim_start();
    if text.starts_with(USER_PREFIX) || has_error_at(text) {
        return Some(UserLine {
            text: text.to_string(),
            heuristic: false,
        });
    }
    if !is_user_error(text) {
        return None;
    }
    // Located payloads keep the location first so editors link them.
    if let Some((loc, rest)) = split_loc(text) {
        return Some(UserLine {
            text: format!("{loc}: error: {rest}"),
            heuristic: true,
        });
    }
    Some(UserLine {
        text: format!("{USER_PREFIX} {text}"),
        heuristic: true,
    })
}

/// A user error with no source location. Exits 1, never an ICE.
pub fn error(msg: &str) -> ! {
    let text = msg.strip_prefix("epic-cc: ").unwrap_or(msg);
    eprintln!("{USER_PREFIX} {text}");
    std::process::exit(1);
}

/// A user error at a source location, so editors can link it. Exits 1.
pub fn error_at(file: &str, line: u32, col: u32, msg: &str) -> ! {
    let text = msg.strip_prefix("epic-cc: ").unwrap_or(msg);
    let text = text.strip_prefix("error: ").unwrap_or(text);
    eprintln!("{file}:{line}:{col}: error: {text}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_payloads_are_user_errors() {
        assert!(is_user_error("epic-cc: error: unknown field 'wat'"));
        assert!(is_user_error("  epic-cc: error: xtal_hz=<Hz> is required"));
    }

    #[test]
    fn located_errors_are_user_errors() {
        assert!(is_user_error(
            "main.c:3:1: error: more than one EPIC_CONFIG"
        ));
        assert!(!is_user_error("main.c: error: no line info"));
    }

    #[test]
    fn backend_unsupported_input_stays_an_error() {
        assert!(is_user_error(
            "isel: GEP-derived pointers are not supported; operand %1 is derived"
        ));
        assert!(is_user_error("callgraph: recursion detected (call cycle)"));
        assert!(is_user_error(
            "rec.c:4:51: callgraph: recursion detected (call cycle involving f)"
        ));
        assert!(is_user_error("callgraph: depth 9 exceeds hardware stack 8"));
        assert!(is_user_error(
            "wholeprog: expected exactly one `main`, found 0"
        ));
        assert!(is_user_error(
            "wholeprog: undefined symbols: frobnicate (called at undef.c:2:18)"
        ));
        assert!(is_user_error(
            "alloc: no arrangement of 355 global(s) fits p16f877a's 4 GPR bank window(s)"
        ));
        assert!(is_user_error(
            "isel: post-banking page-fit failure: f spans pages (0x1000-0x1800)"
        ));
        assert!(is_user_error(
            "asm: program of 8200 words exceeds device flash (highest address 0x4000)"
        ));
    }

    #[test]
    fn plain_bug_payloads_are_internal_errors() {
        assert!(!is_user_error("isel: no slot for main::x"));
        assert!(!is_user_error("index out of bounds: the len is 4"));
        assert!(!is_user_error("wholeprog: no functions in module"));
        assert!(!is_user_error("alloc: unrecognized callgraph line: foo"));
        assert!(!is_user_error("asm: file register 0xFF out of range"));
        // Backend invariant, not input: the scale comes from lowering.
        assert!(!is_user_error("isel-pic18: MULWF scale 300 exceeds 255"));
    }

    #[test]
    fn oversized_baseline_programs_stay_errors() {
        assert!(is_user_error("isel: slot offset 300 exceeds 255"));
        assert!(is_user_error("isel: baseline const index 300 exceeds 255"));
    }

    #[test]
    fn located_user_errors_keep_the_location_first() {
        let line =
            user_error_line("rec.c:4:51: callgraph: recursion detected (call cycle involving f)")
                .expect("located recursion is a user error");
        assert_eq!(
            line.text,
            "rec.c:4:51: error: callgraph: recursion detected (call cycle involving f)"
        );
        assert!(line.heuristic);
    }

    #[test]
    fn unlocated_user_errors_carry_the_tool_mark() {
        let line = user_error_line("callgraph: depth 9 exceeds hardware stack 8")
            .expect("depth is a user error");
        assert_eq!(
            line.text,
            "epic-cc: error: callgraph: depth 9 exceeds hardware stack 8"
        );
        assert!(line.heuristic);
    }

    #[test]
    fn marked_errors_print_verbatim() {
        let line = user_error_line("epic-cc: error: unknown field 'wat'")
            .expect("marked error is a user error");
        assert_eq!(line.text, "epic-cc: error: unknown field 'wat'");
        assert!(!line.heuristic);
    }

    #[test]
    fn internal_block_names_version_and_report_url() {
        let text = format_internal_error("boom", "0.3.0+abc", Some("src/x.rs:1:2"));
        assert!(text.contains("epic-cc: internal compiler error: boom"));
        assert!(text.contains("version: epic-cc 0.3.0+abc"));
        assert!(text.contains(ISSUE_URL));
        assert!(text.contains("epic-cc --version"));
    }
}
