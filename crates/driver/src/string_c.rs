//! The freestanding `<string.h>` implementation, compiled as an extra
//! translation unit when a source includes the header.
//!
//! Index loops where the trip count is known up front (memcpy family,
//! strlen): the integer phi is the shape both backends walk. `strcmp`
//! instead runs pointer induction with a bitwise-`&` latch, which clang
//! keeps as one block over two `phi ptr`: the PIC18 pointer walk's
//! shape, with the last bytes live for the exit subtract.

/// Names defined by one `STRING_C` definition line, or `None` for any
/// other line. The source is ours and every definition opens with the
/// same `__attribute__((noinline))` line, so a line scan is exact here
/// where it would be fragile on arbitrary C.
fn definition_name(line: &str) -> Option<&str> {
    if !line.starts_with("__attribute__((noinline))") {
        return None;
    }
    let before_paren = line.split('(').nth(2)?;
    let name = before_paren
        .split_whitespace()
        .last()?
        .trim_start_matches('*');
    (!name.is_empty()).then_some(name)
}

/// Every function `STRING_C` defines, in source order. `strcmp` appears
/// twice, once per core branch, so callers deduplicate if they care.
pub fn function_names(source: &str) -> Vec<&str> {
    source.lines().filter_map(definition_name).collect()
}

/// `source` with every definition of a name in `names` removed. The
/// `#ifdef` lines around the two `strcmp` branches stay, so removing a
/// name present in both branches still leaves valid preprocessor text,
/// and surviving definitions call the user's name through the header
/// declarations as an ordinary external reference.
pub fn without_functions(source: &str, names: &std::collections::HashSet<String>) -> String {
    let mut out = String::with_capacity(source.len());
    let mut skipping = false;
    for line in source.lines() {
        if skipping {
            if line == "}" {
                skipping = false;
            }
            continue;
        }
        if definition_name(line).is_some_and(|n| names.contains(n)) {
            skipping = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Number of parameters in the parenthesised list opening at `open`,
/// or `None` when it never closes. Nested parens keep their commas.
fn arity_of(line: &str, open: usize) -> Option<usize> {
    let inner = &line[open + 1..];
    let mut depth = 0i32;
    let mut close = None;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                close = Some(i);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let inner = &inner[..close?];
    if inner.trim().is_empty() {
        return Some(0);
    }
    let mut depth = 0i32;
    let mut count = 1usize;
    for c in inner.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => count += 1,
            _ => {}
        }
    }
    Some(count)
}

/// Parameter counts of every `STRING_C` definition, keyed by name.
pub fn function_arities(source: &str) -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    for line in source.lines() {
        if let Some(name) = definition_name(line) {
            if let Some(open) = line.rfind('(') {
                if let Some(arity) = arity_of(line, open) {
                    out.insert(name.to_string(), arity);
                }
            }
        }
    }
    out
}

/// Function names and parameter counts a compiled user unit defines at
/// external linkage, read from its `define` lines. Internal, private,
/// weak and linkonce definitions are not collisions the injected unit
/// must defer to: llvm-link renames the internal ones and keeps the
/// strong definition over a weak or linkonce one, so deferring would
/// invert the linker's own precedence.
pub fn externally_defined(ll_text: &str) -> std::collections::HashMap<String, usize> {
    let mut names = std::collections::HashMap::new();
    for line in ll_text.lines() {
        let Some(rest) = line.strip_prefix("define ") else {
            continue;
        };
        if rest.split_whitespace().any(|t| {
            matches!(
                t,
                "internal"
                    | "private"
                    | "weak"
                    | "weak_odr"
                    | "extern_weak"
                    | "linkonce"
                    | "linkonce_odr"
                    | "available_externally"
            )
        }) {
            continue;
        }
        let Some(at) = rest.find('@') else { continue };
        let after = &rest[at + 1..];
        let name: String = if let Some(quoted) = after.strip_prefix('"') {
            quoted.chars().take_while(|c| *c != '"').collect()
        } else {
            after
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '-'))
                .collect()
        };
        if name.is_empty() {
            continue;
        }
        let Some(open) = after.find('(') else {
            continue;
        };
        if let Some(arity) = arity_of(after, open) {
            names.entry(name).or_insert(arity);
        }
    }
    names
}

pub const STRING_C: &str = r#"#include <stddef.h>
#include <string.h>

__attribute__((noinline)) void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char*)dest;
    const unsigned char *s = (const unsigned char*)src;
    for (size_t i = 0; i < n; i++) d[i] = s[i];
    return dest;
}

__attribute__((noinline)) void *memmove(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char*)dest;
    const unsigned char *s = (const unsigned char*)src;
    if (n == 0) return dest;
    if (d < s) {
        for (size_t i = 0; i < n; i++) d[i] = s[i];
    } else if (d > s) {
        size_t i = n;
        while (i > 0) { i--; d[i] = s[i]; }
    }
    return dest;
}

__attribute__((noinline)) void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char*)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}

__attribute__((noinline)) int memcmp(const void *s1, const void *s2, size_t n) {
    const unsigned char *p1 = (const unsigned char*)s1;
    const unsigned char *p2 = (const unsigned char*)s2;
    for (size_t i = 0; i < n; i++) {
        if (p1[i] != p2[i]) return (int)p1[i] - (int)p2[i];
    }
    return 0;
}

__attribute__((noinline)) size_t strlen(const char *s) {
    size_t i = 0;
    while (s[i]) i++;
    return i;
}

__attribute__((noinline)) size_t strnlen(const char *s, size_t maxlen) {
    size_t i = 0;
    while (i < maxlen && s[i]) i++;
    return i;
}

__attribute__((noinline)) char *strcpy(char *dest, const char *src) {
    size_t i = 0;
    while (src[i]) { dest[i] = src[i]; i++; }
    dest[i] = 0;
    return dest;
}

__attribute__((noinline)) char *strncpy(char *dest, const char *src, size_t n) {
    size_t i = 0;
    while (i < n && src[i]) { dest[i] = src[i]; i++; }
    while (i < n) { dest[i] = 0; i++; }
    return dest;
}

__attribute__((noinline)) char *strcat(char *dest, const char *src) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

__attribute__((noinline)) char *strncat(char *dest, const char *src, size_t n) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (i < n && src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

/* Bitwise `&`, not `&&`: one latch block over two pointer phis after
 * LSR, which the PIC18 pointer walk lowers to `POSTINC` scans. A
 * short-circuit latch splits the loop and defeats it. PIC14 keeps the
 * index form: its backend has no pointer walk and counts the pointer
 * version slower. */
#ifdef _PIC18
__attribute__((noinline)) int strcmp(const char *s1, const char *s2) {
    size_t i = 0;
    while ((((unsigned char)s1[i] == (unsigned char)s2[i])
        & ((unsigned char)s1[i] != 0))) i++;
    return (int)(unsigned char)s1[i] - (int)(unsigned char)s2[i];
}
#else
__attribute__((noinline)) int strcmp(const char *s1, const char *s2) {
    size_t i = 0;
    while (s1[i] && s1[i] == s2[i]) i++;
    return (int)(unsigned char)s1[i] - (int)(unsigned char)s2[i];
}
#endif

__attribute__((noinline)) int strncmp(const char *s1, const char *s2, size_t n) {
    size_t i = 0;
    while (i < n) {
        unsigned char a = (unsigned char)s1[i];
        unsigned char b = (unsigned char)s2[i];
        if (a != b) return (int)a - (int)b;
        if (a == 0) return 0;
        i++;
    }
    return 0;
}

__attribute__((noinline)) void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = (const unsigned char*)s;
    for (size_t i = 0; i < n; i++) {
        if (p[i] == (unsigned char)c) return (void*)(p + i);
    }
    return 0;
}

__attribute__((noinline)) char *strchr(const char *s, int c) {
    size_t i = 0;
    while (1) {
        unsigned char ch = s[i];
        if (ch == (unsigned char)c) return (char*)(s + i);
        if (ch == 0) return 0;
        i++;
    }
}

__attribute__((noinline)) char *strrchr(const char *s, int c) {
    char *last = 0;
    size_t i = 0;
    while (1) {
        unsigned char ch = s[i];
        if (ch == 0) break;
        if (ch == (unsigned char)c) last = (char*)(s + i);
        i++;
    }
    if ((unsigned char)c == 0) return (char*)(s + i);
    return last;
}

__attribute__((noinline)) char *strstr(const char *haystack, const char *needle) {
    size_t nlen = strlen(needle);
    if (nlen == 0) return (char*)haystack;
    for (size_t i = 0; haystack[i]; i++) {
        if (haystack[i] == needle[0] && memcmp(haystack + i, needle, nlen) == 0)
            return (char*)(haystack + i);
    }
    return 0;
}

"#;

/// A memset-only unit, linked when a translation unit emits the
/// `llvm.memset` intrinsic (struct zero-initialization, `__builtin_memset`)
/// without including `<string.h>` (epic-cc#1448). Separate from `STRING_C`
/// so a program defining other string functions itself gains no duplicate
/// definitions from the rescue unit.
pub const MEMSET_C: &str = r#"#include <stddef.h>

__attribute__((noinline)) void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char*)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}
"#;
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_cover_every_string_c_definition() {
        let names = function_names(STRING_C);
        assert_eq!(names.len(), 17, "{names:?}");
        for expected in [
            "memcpy", "memmove", "memset", "memcmp", "strlen", "strnlen", "strcpy", "strncpy",
            "strcat", "strncat", "strcmp", "strncmp", "memchr", "strchr", "strrchr", "strstr",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn removing_memset_keeps_every_other_definition() {
        let reduced = without_functions(STRING_C, &HashSet::from(["memset".to_string()]));
        assert!(!function_names(&reduced).contains(&"memset"));
        assert_eq!(function_names(&reduced).len(), 16);
        assert!(reduced.contains("#include <string.h>"));
    }

    #[test]
    fn removing_strcmp_removes_both_core_branches() {
        let reduced = without_functions(STRING_C, &HashSet::from(["strcmp".to_string()]));
        assert!(!function_names(&reduced).contains(&"strcmp"));
        assert!(reduced.contains("#ifdef _PIC18"));
        assert!(reduced.contains("#else"));
        assert!(reduced.contains("#endif"));
    }

    #[test]
    fn removing_nothing_is_byte_identical() {
        assert_eq!(without_functions(STRING_C, &HashSet::new()), STRING_C);
    }

    #[test]
    fn externally_defined_reads_define_lines_only() {
        let ll = "declare ptr @memset(ptr, i16, i16)\n\
                  define dso_local ptr @memset(ptr %s, i16 %c, i16 %n) {\n}\n\
                  define internal ptr @helper(ptr %s) {\n}\n\
                  define weak ptr @wobble(ptr %s) {\n}\n\
                  define ptr @_Z3fooPc(ptr %s) {\n}\n";
        let names = externally_defined(ll);
        assert_eq!(names.get("memset"), Some(&3));
        assert_eq!(names.get("_Z3fooPc"), Some(&1));
        assert!(!names.contains_key("helper"));
        assert!(!names.contains_key("wobble"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn function_arities_match_the_library_signatures() {
        let arities = function_arities(STRING_C);
        assert_eq!(arities.get("memset"), Some(&3));
        assert_eq!(arities.get("strlen"), Some(&1));
        assert_eq!(arities.get("strcmp"), Some(&2));
        assert_eq!(arities.len(), 16);
    }
}
