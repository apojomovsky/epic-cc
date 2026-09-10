pub const MALLOC_H: &str = r#"#ifndef _MALLOC_H
#define _MALLOC_H

#include <stddef.h>

/* Source compatibility with SDCC's pic16 <malloc.h>: there _MALLOC_SPEC
 * qualifies heap pointers with a memory space. epic-cc has no address
 * spaces, so it is empty here and heap pointers are plain. */

#define _MALLOC_SPEC

void *malloc(size_t n);
void free(void *p);
void _initHeap(unsigned char *dHeap, unsigned int heapsize);

#endif /* _MALLOC_H */
"#;
