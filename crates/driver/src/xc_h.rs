//! The `xc.h` stub epic-cc ships to user code. The driver predefines
//! `__XC8` (the XC8-compat toolchain macro), so vendored third-party
//! sources that guard an `#include <xc.h>` on `__XC8` (m-stack's mmc.c)
//! resolve it here. Nothing in those sources uses anything from the real
//! XC8 header (no SFR names, no `__delay_*`, no interrupt keyword); the
//! include is a compiler allowlist, not a dependency, so an empty stub is
//! the whole contract.

pub const XC_H: &str = r#"#ifndef _XC_H
#define _XC_H

/* epic-cc stub: the real XC8 header is license-gated and its contents are
 * unused by the sources that include it under __XC8. */

#endif /* _XC_H */
"#;
