//! The freestanding `<stdio.h>` implementation, compiled as an extra
//! translation unit when a source includes the header (epic-cc#131).
//!
//! The character sink is the user-provided `putchar` (the HAL's USART or
//! the harness); the formatter never touches hardware. Written per
//! ADR-018's pointer rule: every string walk is an INDEX loop, the only
//! pointer read is `va_arg` (now modelled directly), and the 32-bit
//! conversion bodies are `noinline` helpers so every function stays under
//! the 877A's 2048-word page limit. No floats (`%f` is CC-5).

pub const STDIO_C: &str = r#"#include <stdarg.h>
#include <stddef.h>

extern int putchar(int c);

static int out_cstr(const char *s) {
    int w = 0;
    size_t i = 0;
    while (s[i] != 0) { putchar((unsigned char)s[i]); i++; w++; }
    return w;
}

/* Emit the decimal digits of v (leading zeros dropped), returning the
   digit count. One 32-bit division per digit, with the remainder via
   q*10 (cheaper than a second division). */
__attribute__((noinline)) static int out_dec(unsigned long v, int neg) {
    unsigned long buf[11];
    int n = 0;
    int k;
    if (neg) { putchar('-'); }
    do {
        unsigned long q = v / 10;
        unsigned long d = v - q * 10;
        buf[n] = d; n++;
        v = q;
    } while (v != 0);
    k = n;
    while (n > 0) { n--; putchar((unsigned char)('0' + buf[n])); }
    return k + (neg ? 1 : 0);
}

/* Emit the hexadecimal digits of v (leading zeros dropped). */
__attribute__((noinline)) static int out_hex(unsigned long v, int lc) {
    unsigned long buf[9];
    int n = 0;
    int k;
    do {
        unsigned long d = v & 0xF;
        buf[n] = d; n++;
        v = v >> 4;
    } while (v != 0);
    k = n;
    while (n > 0) { n--; putchar((unsigned char)(buf[n] < 10 ? '0' + buf[n] : (lc ? 'a' : 'A') + (buf[n] - 10))); }
    return k;
}

__attribute__((noinline)) int vprintf(const char *fmt, va_list ap) {
    int written = 0;
    size_t i = 0;
    for (;;) {
        char c = fmt[i];
        if (c == 0) break;
        if (c != '%') { putchar((unsigned char)c); written++; i++; continue; }
        i++;
        int is_long = 0;
        if (fmt[i] == 'l') { is_long = 1; i++; }
        char conv = fmt[i];
        i++;
        switch (conv) {
            case '%': putchar('%'); written++; break;
            case 'c': putchar((unsigned char)va_arg(ap, int)); written++; break;
            case 's': {
                const char *s = va_arg(ap, const char *);
                if (s == 0) { written += out_cstr("(null)"); }
                else { written += out_cstr(s); }
                break;
            }
            case 'u': {
                unsigned long v;
                if (is_long) { v = va_arg(ap, unsigned long); }
                else { v = va_arg(ap, unsigned int); }
                written += out_dec(v, 0);
                break;
            }
            case 'd': case 'i': {
                long v;
                int neg = 0;
                if (is_long) { v = va_arg(ap, long); }
                else { v = va_arg(ap, int); }
                if (v < 0) { v = -v; neg = 1; }
                written += out_dec((unsigned long)v, neg);
                break;
            }
            case 'x': case 'X': {
                unsigned long v;
                if (is_long) { v = va_arg(ap, unsigned long); }
                else { v = va_arg(ap, unsigned int); }
                written += out_hex(v, conv == 'x');
                break;
            }
            default:
                /* Unknown conversion: emit verbatim so a format bug is
                   visible instead of silently dropped. */
                putchar('%'); putchar((unsigned char)conv); written += 2;
                break;
        }
    }
    return written;
}

__attribute__((noinline)) int printf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int w = vprintf(fmt, ap);
    va_end(ap);
    return w;
}
"#;
