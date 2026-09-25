// Definition TU for the gate-A reads: the sized table the `extern`
// declaration in `extern_array_walk.c` refers to.

struct Item {
    unsigned char flags;
    unsigned int value;
};

const struct Item table[] = {
    {11u, 101u},
    {22u, 202u},
    {33u, 303u},
    {44u, 404u},
    {55u, 505u},
};
