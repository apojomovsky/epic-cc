// epic-cc#678: LSR pointer walks over pointer parameters. Loop-reduce
// rewrites `b[off+i]` walks into a pointer phi (`%lsr.iv = phi ptr ...`)
// whose entry arm is a GEP over the param and whose latch arm is the
// walk increment. iselcore resolves the chain only if a GEP over the
// not-yet-seeded phi waits for phi seeding instead of panicking, and
// isel-pic18 materializes the walk slot copy, including a scaled
// single-term move for the stride-2 `unsigned short` walk.
//
// The walk bases come from a volatile select over bare globals, so
// interprocedural optimization cannot globalize them into the callees:
// the pointer params (and their LSR phis) survive to isel. At power-on
// the select is 0, so the first buffer of each pair is used and the
// second must stay zero, which also proves the walks never stray.
// Trip counts stay runtime-unknown (read off the volatile seed) so
// loop-reduce emits pointer walks instead of unrolling: at power-on
// they are 8, 6, 8 and 8.
//
// Hand computation (seed 0, select 0):
//   g_buf[i] == i for i in 0..24, g_buf[24..32] == 0, g_buf2 all zero
//   g_s1 == 0+1+2+3+4+5 == 15
//   g_w[i] == 3*i+1, g_w2 all zero
//   g_s2 == sum of g_w[0..7] == 70
//   main folds both sums into the volatile sink so whole-program
//   dead-store elimination cannot delete them:
//   g_done == ((15 + 70) & 0xFF) ^ 0xA5 == 0xF0, then halts.

static unsigned char g_buf[40];
static unsigned char g_buf2[40];
static unsigned short g_w[8];
static unsigned short g_w2[8];
volatile unsigned short g_s1;
volatile unsigned short g_s2;
volatile unsigned char g_seed;
volatile unsigned char g_sel;
volatile unsigned char g_done;

__attribute__((noinline)) static void zero_walk(unsigned char *b, unsigned char off,
                                                unsigned char n)
{
    for (unsigned char i = 0u; i < n; i++)
    {
        b[(unsigned char)(off + i)] = 0u;
    }
}

__attribute__((noinline)) static unsigned short strided_sum(const unsigned char *b,
                                                            unsigned short off,
                                                            unsigned short n)
{
    unsigned short acc = 0u;
    for (unsigned short i = 0u; i < n; i++)
    {
        acc = (unsigned short)(acc + b[(unsigned short)(off + i)]);
    }
    return acc;
}

__attribute__((noinline)) static void fill16(unsigned short *w, unsigned char n,
                                             unsigned char seed)
{
    for (unsigned char i = 0u; i < n; i++)
    {
        w[i] = (unsigned short)(((unsigned short)seed << 8) | (unsigned short)(3u * i + 1u));
    }
}

__attribute__((noinline)) static unsigned short sum16(const unsigned short *w,
                                                      unsigned short off,
                                                      unsigned char n)
{
    unsigned short acc = 0u;
    for (unsigned char i = 0u; i < n; i++)
    {
        acc = (unsigned short)(acc + w[(unsigned char)(off + i)]);
    }
    return acc;
}

void main(void)
{
    unsigned char seed = g_seed;
    unsigned short start = (unsigned short)(seed & 7u);
    unsigned char *b = ((g_sel & 1u) != 0u) ? g_buf2 : g_buf;
    unsigned short *w = ((g_sel & 1u) != 0u) ? g_w2 : g_w;
    /* Trip counts stay runtime-unknown (read off the volatile seed) so
    loop-reduce emits pointer walks instead of unrolling: at power-on
    they are 8, 6, 8 and 8. */
    unsigned char n8 = (unsigned char)(8u + (seed & 1u));
    unsigned short n6 = (unsigned short)(6u + (seed & 1u));
    unsigned char m8 = (unsigned char)(8u - (seed & 1u));
    /* A dynamic word offset plus a dynamic count that always stay in
    bounds: at power-on they are 0 and 7. */
    unsigned short off2 = (unsigned short)(seed & 1u);
    unsigned char n7 = (unsigned char)(7u - (seed & 1u));
    for (unsigned char i = 0u; i < 32u; i++)
    {
        b[i] = (unsigned char)(seed + i);
    }
    zero_walk(b, 24u, n8);
    g_s1 = strided_sum(b, start, n6);
    fill16(w, m8, seed);
    g_s2 = sum16(w, off2, n7);
    g_done = (unsigned char)(((g_s1 + g_s2) & 0xFFu) ^ 0xA5u);
}
