pub const STDLIB_H: &str = r#"#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>

/* Freestanding stdlib.h for epic-cc. size_t and NULL ride on stddef.h.
 * malloc/free are real code (stdlib_c.rs), linked when this header is
 * included, the same gating the string and stdio units use. */

void *malloc(size_t n);
void free(void *p);

#endif /* _STDLIB_H */
"#;
