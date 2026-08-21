//! A cheap, clang-free scan for `EPIC_CONFIG("...")`'s argument, run before
//! any clang invocation so EPIC_FOSC_HZ can be added to every `-D` list
//! from the start (docs/31 D-10). Comment- and string-literal-aware so a
//! fuse string or a stray comment cannot make it misfire.

/// Scan every source file's raw text for exactly one top-level
/// `EPIC_CONFIG("...")` invocation, skipping `//` and `/* */` comments and
/// `"..."` string literals along the way. Returns the quoted argument, or
/// `None` if no invocation was found anywhere.
///
/// Panics if more than one invocation is found across all files: v1
/// supports exactly one, unconditional, per docs/31 D-10.
pub fn find_epic_config(sources: &[(String, String)]) -> Option<String> {
    let mut found: Option<(String, String)> = None; // (file, spec)
    for (file, text) in sources {
        for spec in find_in_one_file(text) {
            if let Some((prev_file, _)) = &found {
                panic!(
                    "epic-cc: more than one EPIC_CONFIG(...) invocation found \
                     ({prev_file} and {file}); exactly one is supported"
                );
            }
            found = Some((file.clone(), spec));
        }
    }
    found.map(|(_, spec)| spec)
}

fn find_in_one_file(text: &str) -> Vec<String> {
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
                        out.push(rest[..end].to_string());
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
