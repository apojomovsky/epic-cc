#include "gep_overflow_1487.h"

/* Fills the framer buffer through the port, then decodes it in place:
   the caller-side half of the epic-cc#1487 shape, in its own unit so
   the merge inlines decode over the rx field pointer. */
unsigned char poll(struct framer *f, unsigned short max)
{
    unsigned short out_len = 0u;
    unsigned short n = port_read(f->rx, max);
    f->len = n;
    if (decode(f->rx, f->len, &out_len) != 0u) {
        return 0xEEu;
    }
    return f->rx[f->len - out_len];
}
