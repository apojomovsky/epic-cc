#include "gep_overflow_1487.h"

/* A backward in-place decode, distilled from r2s_rcobs_decode: the
   trailing-byte check `buf[len - 1]` is the access clang reassociates
   into `(rx + len) - 1` once this unit inlines over the field base,
   which is the GEP chain that overflowed (epic-cc#1487). */
unsigned char decode(unsigned char *buf, unsigned short len, unsigned short *out_len)
{
    unsigned short read = len;
    unsigned short write = len;
    unsigned char seen = 0u;
    if (len == 0u) {
        *out_len = 0u;
        return 0u;
    }
    if (buf[len - 1u] == 0xFFu) {
        return 1u;
    }
    for (unsigned short i = 0u; i < len; i++) {
        if (buf[i] == 0u) {
            return 3u;
        }
    }
    while (read > 0u) {
        unsigned char code = buf[read - 1u];
        if ((unsigned short)code > read) {
            return 2u;
        }
        if (seen != 0u && code != 0xFFu) {
            buf[--write] = 0u;
        }
        for (unsigned char k = 1u; k < code; k++) {
            buf[--write] = buf[read - 1u - k];
        }
        read = (unsigned short)(read - code);
        seen = 1u;
    }
    *out_len = (unsigned short)(len - write);
    return 0u;
}
