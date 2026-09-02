//! The `stdio.h` stub epic-cc ships to user code. Vendored third-party
//! sources (m-stack's crc.c) include it unconditionally but use nothing
//! from it: the only stdio calls sit under `#if PC_CODE_TO_GENERATE_THE_TABLES`,
//! a PC-only table generator that is never defined on target. The real
//! stdio surface (printf and friends) is epic-cc#131's scope; this stub
//! only makes the include resolve.

pub const STDIO_H: &str = r#"#ifndef _STDIO_H
#define _STDIO_H

/* epic-cc stub: no stdio surface yet (epic-cc#131). */

#endif /* _STDIO_H */
"#;
