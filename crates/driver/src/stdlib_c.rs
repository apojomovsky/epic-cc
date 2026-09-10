//! The freestanding `malloc`/`free`/`_initHeap` implementation, compiled
//! as an extra translation unit when a source includes `<stdlib.h>` or
//! `<malloc.h>` (epic-cc#343).
//!
//! First-fit over a caller-provided arena (`_initHeap` adopts it, the
//! SDCC pic16 model) with out-of-band integer metadata, so the C stays
//! inside the pointer shapes both backends lower (index loops,
//! GEP-derived returns, slot-held comparison; no `ptrtoint` anywhere).
//! `NULL` is the sole out-of-memory signal: `malloc` before any
//! `_initHeap` fails, `malloc(0)` yields a 1-byte block, requests past
//! the arena fail, and `free` on an unknown pointer (including `NULL`,
//! which matches no block) is a no-op. Static state assumes
//! zero-initialized BSS, the same contract every other static in the
//! compiler relies on.

pub const STDLIB_C: &str = r#"#include <stddef.h>
#include <stdlib.h>

#define HEAP_BLOCKS 8
#define HEAP_MAX 255

static unsigned char *heap_base;
static unsigned char heap_cap;
static unsigned char heap_end;
static unsigned char blk_start[HEAP_BLOCKS];
static unsigned char blk_len[HEAP_BLOCKS];
static unsigned char blk_used[HEAP_BLOCKS];

__attribute__((noinline)) void _initHeap(unsigned char *dHeap, unsigned int heapsize) {
    unsigned char i;
    heap_base = dHeap;
    heap_cap = heapsize > HEAP_MAX ? HEAP_MAX : (unsigned char)heapsize;
    heap_end = 0;
    for (i = 0; i < HEAP_BLOCKS; i++) {
        blk_start[i] = 0;
        blk_len[i] = 0;
        blk_used[i] = 0;
    }
}

__attribute__((noinline)) void *malloc(size_t n) {
    unsigned char i;
    unsigned char size;
    if (heap_base == 0) return 0;
    if (n == 0) n = 1;
    if (n > heap_cap) return 0;
    size = (unsigned char)n;
    for (i = 0; i < HEAP_BLOCKS; i++) {
        if (blk_used[i] == 0 && blk_len[i] >= size) {
            blk_used[i] = 1;
            return &heap_base[blk_start[i]];
        }
    }
    for (i = 0; i < HEAP_BLOCKS; i++) {
        if (blk_used[i] == 0 && blk_len[i] == 0) {
            /* Plain int comparison: heap_end + size can exceed 255
             * on large arenas, where a narrowed compare would wrap
             * below the cap and corrupt the heap. */
            if (heap_end + size > heap_cap) return 0;
            blk_start[i] = heap_end;
            blk_len[i] = size;
            blk_used[i] = 1;
            heap_end = (unsigned char)(heap_end + size);
            return &heap_base[blk_start[i]];
        }
    }
    return 0;
}

/* Free routes each candidate address through a volatile slot: PIC18
 * lowers icmp only on slot-held values and a folded GEP has no slot,
 * so comparing against &heap[..] inline panics in isel. The volatile
 * store/reload materializes the address into a slot, the same model
 * a call return relies on. */
__attribute__((noinline)) void free(void *p) {
    unsigned char i;
    unsigned char *volatile probe;
    if (p == 0) return;
    for (i = 0; i < HEAP_BLOCKS; i++) {
        probe = &heap_base[blk_start[i]];
        if (blk_used[i] != 0 && probe == (unsigned char *)p) {
            blk_used[i] = 0;
            return;
        }
    }
}
"#;
