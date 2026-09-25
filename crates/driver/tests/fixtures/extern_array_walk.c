// Gate A for epic-cc#679: an `extern`-declared incomplete array
// (lowered as `[0 x T]` in TUs that only see the declaration) reads
// correctly through a runtime index. `extern_array_defs.c` defines the
// table; this TU only sees the `extern` declaration, so its GEPs fold
// against the zero-length form and isel resolves them against the
// merged definition. Mirrors the real `irq_table[irq]` shape: one
// dynamic index per call, field load, no loop walk.

struct Item {
    unsigned char flags;
    unsigned int value;
};

extern const struct Item table[];

__attribute__((noinline)) unsigned char get_flag(unsigned char irq)
{
    const struct Item *d = &table[irq];
    return d->flags;
}

__attribute__((noinline)) unsigned int get_value(unsigned char irq)
{
    const struct Item *d = &table[irq];
    return d->value;
}
