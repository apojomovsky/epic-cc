//! Did a build actually pull in a given header, once every `#ifdef` is
//! resolved?
//!
//! `need_string`/`need_stdio` used to grep the raw source text for
//! `#include ... string.h`/`stdio.h`, which does not respect `#ifdef`: a
//! TU that guards the include behind a condition the active build never
//! takes (epic-hal's `#ifndef __EPIC_CC__` pattern, precisely to avoid
//! needing epic-cc's stdio runtime) still triggered the driver's injected
//! `__epic_stdio.c`/`__epic_string.c` (epic-cc#196). clang's own `-MD`
//! dependency output lists exactly the headers a build actually
//! preprocessed in, guards already resolved, so `main.rs` checks that
//! instead of the source text.

/// True when `dep_file_text` (the contents of a clang `-MF` Makefile-style
/// dependency file) names a header ending in `/<name>` or equal to `name`.
/// `-MD` output is whitespace- and `\`-continuation-separated, so a plain
/// token scan is enough; no Makefile target/rule parsing is needed.
pub fn dep_file_includes(dep_file_text: &str, name: &str) -> bool {
    dep_file_text.split_whitespace().any(|tok| {
        let tok = tok.trim_end_matches('\\');
        tok == name || tok.ends_with(&format!("/{name}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_header_at_the_end_of_a_path() {
        let dep = "000.o: /repo/a.c \\\n /tmp/epic-cc-1/include/stdio.h \\\n /repo/a.h\n";
        assert!(dep_file_includes(dep, "stdio.h"));
        assert!(!dep_file_includes(dep, "string.h"));
    }

    #[test]
    fn ignores_an_ifdef_guarded_include_never_taken() {
        // The dep file for a TU that guards `#include <stdio.h>` behind a
        // condition the build never takes simply never names stdio.h.
        let dep = "000.o: /repo/a.c /tmp/epic-cc-1/include/stdint.h\n";
        assert!(!dep_file_includes(dep, "stdio.h"));
    }

    #[test]
    fn a_bare_name_with_no_directory_still_matches() {
        assert!(dep_file_includes("out.o: stdio.h\n", "stdio.h"));
    }

    #[test]
    fn does_not_match_a_similarly_suffixed_filename() {
        assert!(!dep_file_includes("out.o: mystdio.h\n", "stdio.h"));
    }
}
