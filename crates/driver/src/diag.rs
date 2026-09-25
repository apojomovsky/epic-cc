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

/// Whether a panic payload is a deliberate user error, not a bug.
pub fn is_user_error(msg: &str) -> bool {
    let text = msg.trim_start();
    if text.starts_with(USER_PREFIX) {
        return true;
    }
    if has_error_at(text) {
        return true;
    }
    if text.starts_with("callgraph:") {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    lower.contains("not supported") || lower.contains("unsupported")
}

/// Whether `msg` already reads `<file>:<line>:<col>: error: ...`.
fn has_error_at(msg: &str) -> bool {
    let first = msg.lines().next().unwrap_or("");
    let mut parts = first.splitn(4, ':');
    let (Some(_), Some(line), Some(col), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    line.trim().parse::<u32>().is_ok()
        && col.trim().parse::<u32>().is_ok()
        && rest.trim_start().starts_with("error:")
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
        if is_user_error(&msg) {
            eprintln!("{msg}");
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
    }

    #[test]
    fn plain_bug_payloads_are_internal_errors() {
        assert!(!is_user_error("isel: no slot for main::x"));
        assert!(!is_user_error("index out of bounds: the len is 4"));
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
