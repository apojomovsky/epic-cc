//! The `stdarg.h` stub epic-cc ships to user code. The msp430-proxy clang
//! lowers the `__builtin_va_*` intrinsics that back the standard va_list
//! macros to `llvm.va_start`/`va_arg`/`llvm.va_end` IR, which irparse and
//! both backends model directly (epic-cc#131). The header itself is the
//! standard one-liner over the builtins.

pub const STDARG_H: &str = r#"#ifndef _STDARG_H
#define _STDARG_H

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)
#define va_copy(dest, src) __builtin_va_copy(dest, src)

#endif /* _STDARG_H */
"#;
