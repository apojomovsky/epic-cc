//! A cheap, clang-free scan for `EPIC_CONFIG("...")`'s argument and for
//! `#pragma config` settings, run before any clang invocation so
//! EPIC_FOSC_HZ can be added to every `-D` list from the start (docs/31
//! §10). Comment- and string-literal-aware so a fuse string or a stray
//! comment cannot make it misfire. Preprocessor-blind by design: `#if` arms
//! never taken still count, the same contract as the config scanners.
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

/// One `#define _XTAL_FREQ ...` hit with its source site. `value` is the
/// plain integer when the replacement list is one (`4000000`, `0x3D0900`,
/// optionally parenthesized with `U`/`L` suffixes); `None` means defined
/// but computed, which clang still evaluates for the delay macros while
/// the driver cannot use it for agreement.
pub struct XtalFreq {
    pub value: Option<u64>,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// The last live `#define _XTAL_FREQ` across all sources, if any.
/// Comment-, string- and line-aware like the other scanners; definitions
/// and `#undef`s fold in order with last-wins, matching the preprocessor.
pub fn find_xtal_freq(sources: &[(String, String)]) -> Option<XtalFreq> {
    let mut out: Option<XtalFreq> = None;
    for (file, text) in sources {
        for hit in find_xtal_in_one_file(file, text) {
            out = hit;
        }
    }
    out
}

/// One `__delay_ms(` / `__delay_us(` use with its source site, for the
/// error that names the three clock spellings when `_XTAL_FREQ` is absent.
pub struct DelayUse {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// The first delay-macro call across all sources, if any.
pub fn find_delay_use(sources: &[(String, String)]) -> Option<DelayUse> {
    for (file, text) in sources {
        if let Some(hit) = find_delay_in_one_file(file, text) {
            return Some(hit);
        }
    }
    None
}

fn find_xtal_in_one_file(file: &str, text: &str) -> Vec<Option<XtalFreq>> {
    // Events in order: Some on `#define`, None on `#undef`. The caller
    // folds them with last-wins, matching the preprocessor.
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut line_start = true;
    let mut i = 0;
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
            if b[i..j].contains(&b'\n') {
                line_start = true;
            }
            i = j;
            continue;
        }
        if b[i] == b'#' && line_start {
            let mut j = i + 1;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            let directive = read_word(text, b, &mut j);
            if directive == "define" {
                while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                let ns = j;
                while j < b.len() && is_ident(b[j]) {
                    j += 1;
                }
                // A `(` glued to the name is a function-like macro, not
                // the object-like `_XTAL_FREQ` the delays expand.
                if text.get(ns..j) == Some("_XTAL_FREQ") && b.get(j) != Some(&b'(') {
                    let (line, col) = line_col(text, i);
                    let mut k = j;
                    while k < b.len() && b[k] != b'\n' {
                        k += 1;
                    }
                    let value = parse_c_int(&strip_comments(text, &text[j..k]));
                    out.push(Some(XtalFreq {
                        value,
                        file: file.to_string(),
                        line,
                        col,
                    }));
                    i = k;
                    line_start = false;
                    continue;
                }
            } else if directive == "undef" {
                while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                let ns = j;
                while j < b.len() && is_ident(b[j]) {
                    j += 1;
                }
                if text.get(ns..j) == Some("_XTAL_FREQ") {
                    out.push(None);
                }
            }
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        line_start = false;
        i += 1;
    }
    out
}

/// A plain C integer literal: decimal, `0x` hex or `0` octal, one optional
/// parenthesis layer, `'` separators and `U`/`L` suffixes allowed.
/// Anything else (an expression, an empty replacement list) is `None`.
/// Shared with the `-D_XTAL_FREQ=` handling, which feeds clang the same macro.
pub fn parse_c_int(token: &str) -> Option<u64> {
    let mut s = token.trim();
    if s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
        s = s[1..s.len() - 1].trim();
    }
    let s: String = s
        .trim_end_matches(|c: char| c == 'u' || c == 'U' || c == 'l' || c == 'L')
        .chars()
        .filter(|c| *c != '\'')
        .collect();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return u64::from_str_radix(hex, 16).ok();
    }
    if s.len() > 1 && s.starts_with('0') {
        if !s.bytes().all(|c| matches!(c, b'0'..=b'7')) {
            return None;
        }
        return u64::from_str_radix(&s, 8).ok();
    }
    if !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
/// The identifier word at `*j`, advancing past it. Used for directive
/// names on `#` lines.
fn read_word(text: &str, b: &[u8], j: &mut usize) -> String {
    let ns = *j;
    while *j < b.len() && is_ident(b[*j]) {
        *j += 1;
    }
    text.get(ns..*j).unwrap_or("").to_string()
}

/// `line` with comment runs blanked to spaces, so a trailing comment on a
/// `#define` line cannot poison the value parse. (String spans blank out
/// too; any string in the replacement already defeats integer parse.)
fn strip_comments(text: &str, line: &str) -> String {
    let b = text.as_bytes();
    let base = line.as_ptr() as usize - b.as_ptr() as usize;
    let mut out = line.to_string().into_bytes();
    let mut i = 0;
    while i < out.len() {
        if let Some(j) = skip_trivia(&b[base + i..], 0) {
            for k in i..i + j {
                if out[k] != b'\n' {
                    out[k] = b' ';
                }
            }
            i += j;
            continue;
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn find_delay_in_one_file(file: &str, text: &str) -> Option<DelayUse> {
    let b = text.as_bytes();
    let mut i = 0;
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
            i = j;
            continue;
        }
        if b[i] == b'#' && line_start {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'_' || b[i].is_ascii_alphabetic() {
            let ns = i;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            let word = &text[ns..i];
            if word == "__delay_ms" || word == "__delay_us" {
                let mut j = i;
                loop {
                    while j < b.len()
                        && (b[j] == b' ' || b[j] == b'\t' || b[j] == b'\n' || b[j] == b'\r')
                    {
                        j += 1;
                    }
                    if let Some(k) = skip_trivia(b, j) {
                        j = k;
                        continue;
                    }
                    break;
                }
                if b.get(j) == Some(&b'(') {
                    let (line, col) = line_col(text, ns);
                    return Some(DelayUse {
                        name: word.to_string(),
                        file: file.to_string(),
                        line,
                        col,
                    });
                }
            }
            line_start = false;
            continue;
        }
        line_start = false;
        i += 1;
    }
    None
}
