// Two packable static flags (every store writes 0/1, so clang narrows
// each to `global i1`): `alloc` shares one byte for them (epic-cc#1162).
// Set and read live in separate functions so the flags survive as
// globals; `in` is test-driven through the sim so nothing folds.
static unsigned char f_a, f_b;

// Observable outputs stay external: unread internal globals are dead and
// vanish before allocation, which would prove nothing.
unsigned char in;
unsigned char out;

__attribute__((noinline)) void set_flags(void) {
    f_a = 0;
    f_b = 0;
    if (in & 1u) {
        f_a = 1;
    }
    if (in & 2u) {
        f_b = 1;
    }
}

__attribute__((noinline)) unsigned char read_flags(void) {
    unsigned char r = 0;
    if (f_a) {
        r |= 1u;
    }
    if (f_b) {
        r |= 2u;
    }
    f_a = 0;
    f_b = 0;
    if (f_a) {
        r |= 4u;
    }
    if (f_b) {
        r |= 8u;
    }
    return r;
}

void main(void) {
    for (;;) {
        set_flags();
        out = read_flags();
    }
}
