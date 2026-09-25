//! A cheap, clang-free scan for `EPIC_CONFIG("...")`'s argument, run before
//! any clang invocation so EPIC_FOSC_HZ can be added to every `-D` list
//! from the start (docs/31 §10). Comment- and string-literal-aware so a
//! fuse string or a stray comment cannot make it misfire.

use super::diag;

/// One `EPIC_CONFIG("...")` hit with its source site, so config errors can
/// point at it as `file:line:col`.
pub struct FoundConfig {
    /// The quoted argument.
    pub spec: String,
    /// The input file holding it.
    pub file: String,
    /// 1-based line and column of the `EPIC_CONFIG` token.
    pub line: u32,
    pub col: u32,
}

/// Scan every source file's raw text for top-level `EPIC_CONFIG("...")`
/// invocations, skipping `//` and `/* */` comments and `"..."` string
/// literals along the way.
pub fn find_epic_configs(sources: &[(String, String)]) -> Vec<FoundConfig> {
    let mut out = Vec::new();
    for (file, text) in sources {
        for (spec, line, col) in find_in_one_file(text) {
            out.push(FoundConfig {
                spec,
                file: file.clone(),
                line,
                col,
            });
        }
    }
    out
}

/// Scan every source file's raw text for exactly one top-level
/// `EPIC_CONFIG("...")` invocation, skipping `//` and `/* */` comments and
/// `"..."` string literals along the way. Returns the quoted argument, or
/// `None` if no invocation was found anywhere.
///
/// Panics if more than one invocation is found across all files: this
/// supports exactly one, unconditional, per docs/31 §10.
pub fn find_epic_config(sources: &[(String, String)]) -> Option<String> {
    let found = find_epic_configs(sources);
    if found.len() > 1 {
        let first = &found[0];
        let second = &found[1];
        panic!(
            "{}more than one EPIC_CONFIG(...) invocation found \
             ({}:{}:{} and {}:{}:{}); exactly one is supported",
            diag::USER_PREFIX,
            first.file,
            first.line,
            first.col,
            second.file,
            second.line,
            second.col,
        );
    }
    found.into_iter().next().map(|f| f.spec)
}

/// Line and column (both 1-based) of byte offset `idx` in `text`.
fn line_col(text: &str, idx: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for &b in text.as_bytes().iter().take(idx) {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn find_in_one_file(text: &str) -> Vec<(String, u32, u32)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // Skip // line comments.
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Skip /* block comments */.
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Skip "string literals", so a comment delimiter or the word
        // EPIC_CONFIG inside one is not mistaken for real source.
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if text[i..].starts_with("EPIC_CONFIG") {
            let after = &text[i + "EPIC_CONFIG".len()..];
            let trimmed = after.trim_start();
            if let Some(rest) = trimmed.strip_prefix('(') {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        let (line, col) = line_col(text, i);
                        out.push((rest[..end].to_string(), line, col));
                        i += "EPIC_CONFIG".len();
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}
