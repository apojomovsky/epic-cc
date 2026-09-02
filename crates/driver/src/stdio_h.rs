//! The `stdio.h` header epic-cc ships to user code: exactly the entry
//! points the bundled formatter implements (epic-cc#131, ADR-018
//! discipline: a missing entry point is a clang error at the call site,
//! never a link-time surprise). No FILE stream surface: the target has no
//! OS and the retargetable sink is a user-provided `putchar`.

pub const STDIO_H: &str = r#"#ifndef _STDIO_H
#define _STDIO_H

#include <stdarg.h>
#include <stddef.h>

int printf(const char *fmt, ...);
int puts(const char *s);
int putchar(int c);
int snprintf(char *s, size_t n, const char *fmt, ...);
int vsnprintf(char *s, size_t n, const char *fmt, va_list ap);
int sprintf(char *s, const char *fmt, ...);
int vprintf(const char *fmt, va_list ap);

#endif /* _STDIO_H */
"#;
