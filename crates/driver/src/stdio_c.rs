//! The freestanding `<stdio.h>` implementation, compiled as an extra
//! translation unit when a source includes the header (epic-cc#131).
//!
//! The character sink is the user-provided `putchar` (the HAL's USART or
//! the harness); the formatter never touches hardware. The sink contract
//! is `void putchar(char)` (SDCC pic16's prototype, whose `__wparam`
//! the driver predefines empty), not C's `int putchar(int)`, so one
//! corpus source defines putchar for both compilers (docs/35 section 4).
//! Written per ADR-018's pointer rule: every string walk is an INDEX
//! loop, the only pointer read is `va_arg` (now modelled directly), and
//! the 32-bit conversion bodies are `noinline` helpers so every
//! function stays under the 877A's 2048-word page limit.
//! `%f` formats an f32 (== double here) at 2 fixed decimals using the
//! float runtime.

pub const STDIO_C: &str = r#"#include <stdarg.h>
#include <stddef.h>

extern void putchar(char c);

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

/* Format one f32 (== double here) to fixed 2 decimals, returning the
   written count. Pulled out of vprintf into its own noinline frame so the
   float runtime routines stack after this small frame, keeping vprintf's
   frame small enough that vprintf + the float routines fit the PIC18
   access-bank GPR region (the soft-float bodies are access-bank-bound).
   clang folds `-d` to `fneg` (unsupported), so negation is written as
   `0.0 - d`, which comes out as fsub. */
__attribute__((noinline)) static int out_float(double d) {
    int w = 0;
    int neg = 0;
    if (d < 0) { neg = 1; d = 0.0 - d; }
    unsigned long whole = (unsigned long)d;
    double frac = d - (double)whole;
    unsigned long cent = (unsigned long)(frac * 100.0 + 0.5);
    if (cent >= 100) { cent = 0; whole++; }
    if (neg) { putchar('-'); w++; }
    w += out_dec(whole, 0);
    putchar('.'); w++;
    putchar((unsigned char)('0' + cent / 10)); w++;
    putchar((unsigned char)('0' + cent % 10)); w++;
    return w;
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
            case 'f': case 'F':
                written += out_float(va_arg(ap, double));
                break;
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
