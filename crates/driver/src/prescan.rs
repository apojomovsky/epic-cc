//! A cheap, clang-free scan for `EPIC_CONFIG("...")`'s argument and for
//! `#pragma config` settings, run before any clang invocation so
//! EPIC_FOSC_HZ can be added to every `-D` list from the start (docs/31
//! §10). Comment- and string-literal-aware so a fuse string or a stray
//! comment cannot make it misfire.
//!
//! clang drops unknown pragmas before the `.ll`, so `#pragma config NAME
//! = VALUE` never survives to the compiled program: the driver recovers
//! it here and lowers it through the same resolution `EPIC_CONFIG` uses.

/// One `#pragma config NAME = VALUE` setting with its source site.
pub struct PragmaSetting {
    /// The setting and value spellings as written (pack aliases included).
    pub name: String,
    pub value: String,
    /// The input file holding it, and the 1-based line and column of the
    /// `#` that opens the directive.
    pub file: String,
    pub line: u32,
    pub col: u32,
}

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
/// invocations, skipping line and block comments and `"..."` string
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
/// Panics if more than one invocation is found across all files: this
/// supports exactly one, unconditional, per docs/31 §10.
pub fn find_epic_config(sources: &[(String, String)]) -> Option<String> {
    single_epic(find_epic_configs(sources)).map(|f| f.spec)
}

/// The single `EPIC_CONFIG` hit, if any. Panics naming both sites when more
/// than one is found across all files: exactly one unconditional
/// invocation is supported, per docs/31 §10. `find_epic_config` delegates
/// to this; the driver uses it directly to keep the site for mixing errors.
pub fn single_epic(found: Vec<FoundConfig>) -> Option<FoundConfig> {
    if found.len() > 1 {
        let first = &found[0];
        let second = &found[1];
        panic!(
            "{} more than one EPIC_CONFIG(...) invocation found \
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
    found.into_iter().next()
}

/// Every `#pragma config NAME = VALUE` setting with its source site.
/// Pairs may share one directive (`#pragma config A = B, C = D`); the
/// directive keyword match ignores case.
pub fn find_pragma_config(sources: &[(String, String)]) -> Vec<PragmaSetting> {
    let mut out = Vec::new();
    for (file, text) in sources {
        out.extend(find_pragma_in_one_file(file, text));
    }
    out
}

/// Validate pragma settings against the device's config region and join
/// them into one `NAME=VALUE, ...` spec `resolve_config` consumes.
/// Matching is case-insensitive over normalized names and pack aliases,
/// exactly like `EPIC_CONFIG`.
///
/// Panics with a located error on an unknown field or value (naming the
/// valid ones) or on one field set to two different values.
pub fn pragma_spec(region: &device::ConfigRegion, settings: &[PragmaSetting]) -> String {
    let mut seen: Vec<(&str, String, &PragmaSetting)> = Vec::new();
    let mut pairs = Vec::new();
    for s in settings {
        let field = device::find_field(region, &s.name).unwrap_or_else(|| {
            let names: Vec<&str> = region
                .fields
                .iter()
                .flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied()))
                .collect();
            panic!(
                "{}:{}:{}: error: unknown config field '{}' in #pragma config \
                 (expected one of: {})",
                s.file,
                s.line,
                s.col,
                s.name,
                names.join(", ")
            )
        });
        let value = field
            .values
            .iter()
            .find(|v| {
                v.name.eq_ignore_ascii_case(&s.value)
                    || v.aliases.iter().any(|a| a.eq_ignore_ascii_case(&s.value))
            })
            .unwrap_or_else(|| {
                let opts: Vec<&str> = field
                    .values
                    .iter()
                    .flat_map(|v| std::iter::once(v.name).chain(v.aliases.iter().copied()))
                    .collect();
                panic!(
                    "{}:{}:{}: error: unknown config value '{}' for field '{}' in #pragma config \
                     (expected one of: {})",
                    s.file,
                    s.line,
                    s.col,
                    s.value,
                    field.name,
                    opts.join(", ")
                )
            });
        if let Some((_, prev, first)) = seen.iter().find(|(n, _, _)| *n == field.name) {
            if !prev.eq_ignore_ascii_case(value.name) {
                panic!(
                    "{}:{}:{}: error: conflicting #pragma config for field '{}': \
                     '{}' here but '{}' at {}:{}:{}",
                    s.file,
                    s.line,
                    s.col,
                    field.name,
                    s.value,
                    prev,
                    first.file,
                    first.line,
                    first.col,
                );
            }
            continue;
        }
        seen.push((field.name, value.name.to_string(), s));
        pairs.push(format!("{}={}", s.name, s.value));
    }
    pairs.join(", ")
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

/// If offset `i` opens a line comment, a block comment, or a `"..."`
/// string literal, the offset just past it; otherwise `None`.
fn skip_trivia(b: &[u8], i: usize) -> Option<usize> {
    // A `//` line comment runs to the newline.
    if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
        let mut j = i;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        return Some(j);
    }
    // A block comment runs to its closer.
    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
        let mut j = i + 2;
        while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
            j += 1;
        }
        return Some((j + 2).min(b.len()));
    }
    // A string literal hides comment delimiters and config-likes inside
    // it, so neither scanner mistakes quoted text for real source.
    if b[i] == b'"' {
        let mut j = i + 1;
        while j < b.len() && b[j] != b'"' {
            if b[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        return Some((j + 1).min(b.len()));
    }
    None
}

/// An identifier character for config names and values: letters, digits,
/// and underscore, matching the normalized and pack-native spellings.
fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether offset `i` opens a `#pragma config` directive (`#` first on its
/// line, case-insensitive keywords, `config` on a word boundary). Used to
/// tell config directives (parsed or malformed) apart from every other
/// `#` line, which this scanner skips.
fn is_config_pragma(text: &str, b: &[u8], i: usize) -> bool {
    let mut j = i + 1;
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if text[j..]
        .get(..6)
        .is_none_or(|w| !w.eq_ignore_ascii_case("pragma"))
    {
        return false;
    }
    j += "pragma".len();
    if j >= b.len() || !(b[j] == b' ' || b[j] == b'\t') {
        return false;
    }
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if text[j..]
        .get(..6)
        .is_none_or(|w| !w.eq_ignore_ascii_case("config"))
    {
        return false;
    }
    j += "config".len();
    j >= b.len() || !is_ident(b[j])
}

/// If offset `i` opens a `#pragma config` directive (checked first with
/// `is_config_pragma`), the parsed settings with the end offset. `None`
/// means the directive is malformed, which the caller reports.
fn match_pragma(text: &str, b: &[u8], i: usize) -> Option<(Vec<(String, String)>, usize)> {
    if b[i] != b'#' {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if text[j..]
        .get(..6)
        .is_none_or(|w| !w.eq_ignore_ascii_case("pragma"))
    {
        return None;
    }
    j += "pragma".len();
    if j >= b.len() || !(b[j] == b' ' || b[j] == b'\t') {
        return None;
    }
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if text[j..]
        .get(..6)
        .is_none_or(|w| !w.eq_ignore_ascii_case("config"))
    {
        return None;
    }
    j += "config".len();
    if j < b.len() && is_ident(b[j]) {
        return None;
    }
    let mut pairs = Vec::new();
    loop {
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        let ns = j;
        while j < b.len() && is_ident(b[j]) {
            j += 1;
        }
        if ns == j {
            return None;
        }
        let name = text[ns..j].to_string();
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            return None;
        }
        j += 1;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        let vs = j;
        while j < b.len() && is_ident(b[j]) {
            j += 1;
        }
        if vs == j {
            return None;
        }
        pairs.push((name, text[vs..j].to_string()));
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if b.get(j) == Some(&b',') {
            j += 1;
            continue;
        }
        break;
    }
    // Past the last pair only whitespace, a comment, or end of line may
    // follow; anything else is a malformed directive, not a prefix of one.
    let mut k = j;
    loop {
        while k < b.len() && (b[k] == b' ' || b[k] == b'\t' || b[k] == b'\r') {
            k += 1;
        }
        if k >= b.len() || b[k] == b'\n' {
            break;
        }
        if b[k] == b'/' && b.get(k + 1) == Some(&b'/') {
            break;
        }
        if b[k] == b'/' && b.get(k + 1) == Some(&b'*') {
            k += 2;
            while k + 1 < b.len() && !(b[k] == b'*' && b[k + 1] == b'/') {
                if b[k] == b'\n' {
                    break;
                }
                k += 1;
            }
            if k + 1 < b.len() && b[k] == b'*' && b[k + 1] == b'/' {
                k += 2;
                continue;
            }
            break;
        }
        return None;
    }
    Some((pairs, j))
}

fn find_pragma_in_one_file(file: &str, text: &str) -> Vec<PragmaSetting> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // A directive's `#` is the first non-blank character on its line.
    let mut line_start = true;
    while i < b.len() {
        if b[i] == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if b[i] == b' ' || b[i] == b'\t' || b[i] == b'\r' {
            i += 1;
            continue;
        }
        if let Some(j) = skip_trivia(b, i) {
            // A block comment ends mid-line without resetting the
            // directive position: `#` past one still opens a directive.
            if text.as_bytes()[i..j].contains(&b'\n') {
                line_start = true;
            }
            i = j;
            continue;
        }
        if b[i] == b'#' && line_start {
            // Only `#pragma config` belongs to this scanner; every other
            // `#` line (includes, defines, other pragmas) is skipped to
            // end of line.
            if !is_config_pragma(text, b, i) {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            let (line, col) = line_col(text, i);
            match match_pragma(text, b, i) {
                Some((pairs, j)) => {
                    for (name, value) in pairs {
                        out.push(PragmaSetting {
                            name,
                            value,
                            file: file.to_string(),
                            line,
                            col,
                        });
                    }
                    i = j;
                    line_start = false;
                    continue;
                }
                None => panic!(
                    "{file}:{line}:{col}: error: malformed #pragma config \
                     (expected `#pragma config NAME = VALUE`, pairs separated by commas)"
                ),
            }
        }
        line_start = false;
        i += 1;
    }
    out
}
