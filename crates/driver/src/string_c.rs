//! The freestanding `<string.h>` implementation, compiled as an extra
//! translation unit when a source includes the header.
//!
//! Index loops where the trip count is known up front (memcpy family,
//! strlen): the integer phi is the shape both backends walk. `strcmp`
//! instead runs pointer induction with a bitwise-`&` latch, which clang
//! keeps as one block over two `phi ptr`: the PIC18 pointer walk's
//! shape, with the last bytes live for the exit subtract.

pub const STRING_C: &str = r#"#include <stddef.h>
#include <string.h>

__attribute__((noinline)) void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char*)dest;
    const unsigned char *s = (const unsigned char*)src;
    for (size_t i = 0; i < n; i++) d[i] = s[i];
    return dest;
}

__attribute__((noinline)) void *memmove(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char*)dest;
    const unsigned char *s = (const unsigned char*)src;
    if (n == 0) return dest;
    if (d < s) {
        for (size_t i = 0; i < n; i++) d[i] = s[i];
    } else if (d > s) {
        size_t i = n;
        while (i > 0) { i--; d[i] = s[i]; }
    }
    return dest;
}

__attribute__((noinline)) void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char*)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}

__attribute__((noinline)) int memcmp(const void *s1, const void *s2, size_t n) {
    const unsigned char *p1 = (const unsigned char*)s1;
    const unsigned char *p2 = (const unsigned char*)s2;
    for (size_t i = 0; i < n; i++) {
        if (p1[i] != p2[i]) return (int)p1[i] - (int)p2[i];
    }
    return 0;
}

__attribute__((noinline)) size_t strlen(const char *s) {
    size_t i = 0;
    while (s[i]) i++;
    return i;
}

__attribute__((noinline)) size_t strnlen(const char *s, size_t maxlen) {
    size_t i = 0;
    while (i < maxlen && s[i]) i++;
    return i;
}

__attribute__((noinline)) char *strcpy(char *dest, const char *src) {
    size_t i = 0;
    while (src[i]) { dest[i] = src[i]; i++; }
    dest[i] = 0;
    return dest;
}

__attribute__((noinline)) char *strncpy(char *dest, const char *src, size_t n) {
    size_t i = 0;
    while (i < n && src[i]) { dest[i] = src[i]; i++; }
    while (i < n) { dest[i] = 0; i++; }
    return dest;
}

__attribute__((noinline)) char *strcat(char *dest, const char *src) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

__attribute__((noinline)) char *strncat(char *dest, const char *src, size_t n) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (i < n && src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

/* Bitwise `&`, not `&&`: one latch block over two pointer phis after
 * LSR, which the PIC18 pointer walk lowers to `POSTINC` scans. A
 * short-circuit latch splits the loop and defeats it. PIC14 keeps the
 * index form: its backend has no pointer walk and counts the pointer
 * version slower. */
#ifdef _PIC18
__attribute__((noinline)) int strcmp(const char *s1, const char *s2) {
    size_t i = 0;
    while ((((unsigned char)s1[i] == (unsigned char)s2[i])
        & ((unsigned char)s1[i] != 0))) i++;
    return (int)(unsigned char)s1[i] - (int)(unsigned char)s2[i];
}
#else
__attribute__((noinline)) int strcmp(const char *s1, const char *s2) {
    size_t i = 0;
    while (s1[i] && s1[i] == s2[i]) i++;
    return (int)(unsigned char)s1[i] - (int)(unsigned char)s2[i];
}
#endif

__attribute__((noinline)) int strncmp(const char *s1, const char *s2, size_t n) {
    size_t i = 0;
    while (i < n) {
        unsigned char a = (unsigned char)s1[i];
        unsigned char b = (unsigned char)s2[i];
        if (a != b) return (int)a - (int)b;
        if (a == 0) return 0;
        i++;
    }
    return 0;
}

__attribute__((noinline)) void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = (const unsigned char*)s;
    for (size_t i = 0; i < n; i++) {
        if (p[i] == (unsigned char)c) return (void*)(p + i);
    }
    return 0;
}

__attribute__((noinline)) char *strchr(const char *s, int c) {
    size_t i = 0;
    while (1) {
        unsigned char ch = s[i];
        if (ch == (unsigned char)c) return (char*)(s + i);
        if (ch == 0) return 0;
        i++;
    }
}

__attribute__((noinline)) char *strrchr(const char *s, int c) {
    char *last = 0;
    size_t i = 0;
    while (1) {
        unsigned char ch = s[i];
        if (ch == 0) break;
        if (ch == (unsigned char)c) last = (char*)(s + i);
        i++;
    }
    if ((unsigned char)c == 0) return (char*)(s + i);
    return last;
}

__attribute__((noinline)) char *strstr(const char *haystack, const char *needle) {
    size_t nlen = strlen(needle);
    if (nlen == 0) return (char*)haystack;
    for (size_t i = 0; haystack[i]; i++) {
        if (haystack[i] == needle[0] && memcmp(haystack + i, needle, nlen) == 0)
            return (char*)(haystack + i);
    }
    return 0;
}

"#;
