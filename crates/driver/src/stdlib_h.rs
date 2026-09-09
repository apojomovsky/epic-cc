pub const STDLIB_H: &str = r#"#ifndef _STDLIB_H
#define _STDLIB_H

/* Freestanding stdlib.h for epic-cc. Consumers so far need only size_t
 * (m-stack's usb.h prototypes take it). Extends on demand,
 * as for the other builtin headers. */

typedef unsigned int size_t;

#endif /* _STDLIB_H */
"#;
