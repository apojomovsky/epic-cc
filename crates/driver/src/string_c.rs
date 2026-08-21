//! The freestanding `<string.h>` implementation, compiled as an extra
//! translation unit when a source includes the header (CC-2).
//!
//! Written with `size_t` index loops rather than pointer walks: an index keeps
//! the loop-carried value an integer phi, so a pointer only ever materialises
//! as a GEP over a parameter slot, the one pointer shape both backends lower.

pub const STRING_C: &str = r#"#include <stddef.h>
#include <string.h>

void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char*)dest;
    const unsigned char *s = (const unsigned char*)src;
    for (size_t i = 0; i < n; i++) d[i] = s[i];
    return dest;
}

void *memmove(void *dest, const void *src, size_t n) {
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

void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char*)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}

int memcmp(const void *s1, const void *s2, size_t n) {
    const unsigned char *p1 = (const unsigned char*)s1;
    const unsigned char *p2 = (const unsigned char*)s2;
    for (size_t i = 0; i < n; i++) {
        if (p1[i] != p2[i]) return (int)p1[i] - (int)p2[i];
    }
    return 0;
}

size_t strlen(const char *s) {
    size_t i = 0;
    while (s[i]) i++;
    return i;
}

size_t strnlen(const char *s, size_t maxlen) {
    size_t i = 0;
    while (i < maxlen && s[i]) i++;
    return i;
}

char *strcpy(char *dest, const char *src) {
    size_t i = 0;
    while (src[i]) { dest[i] = src[i]; i++; }
    dest[i] = 0;
    return dest;
}

char *strncpy(char *dest, const char *src, size_t n) {
    size_t i = 0;
    while (i < n && src[i]) { dest[i] = src[i]; i++; }
    while (i < n) { dest[i] = 0; i++; }
    return dest;
}

char *strcat(char *dest, const char *src) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

char *strncat(char *dest, const char *src, size_t n) {
    size_t d = 0;
    while (dest[d]) d++;
    size_t i = 0;
    while (i < n && src[i]) { dest[d] = src[i]; d++; i++; }
    dest[d] = 0;
    return dest;
}

int strcmp(const char *s1, const char *s2) {
    size_t i = 0;
    while (s1[i] && s1[i] == s2[i]) i++;
    return (int)(unsigned char)s1[i] - (int)(unsigned char)s2[i];
}

int strncmp(const char *s1, const char *s2, size_t n) {
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


"#;
