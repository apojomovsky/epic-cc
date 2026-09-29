/* Rewrite-B acceptance (epic-cc#639): twelve constant bytes stored
 * field by field, three of them function-pointer halves. The const-runs
 * pass tables the run into one flash table plus a #504 counted copy. */
typedef struct { void (*f0)(void); void (*f1)(void); void (*f2)(void); unsigned char tag; unsigned char extra[5]; } Ops;
typedef unsigned char (*probe_t)(const Ops *);
volatile unsigned char sink;
volatile unsigned char called;
static void fa(void) { called = 0xA1; }
static void fb(void) {}
static void fc(void) {}
static unsigned char probe(const Ops *p) {
    p->f0();
    return (unsigned char)(p->tag ^ p->extra[0] ^ p->extra[4]);
}
volatile probe_t pfn;
void main(void) {
    Ops ops;
    ops.f0 = fa;
    ops.f1 = fb;
    ops.f2 = fc;
    ops.tag = 0x7E;
    ops.extra[0] = 1;
    ops.extra[1] = 2;
    ops.extra[2] = 3;
    ops.extra[3] = 4;
    ops.extra[4] = 5;
    pfn = probe;
    sink = pfn(&ops);
}
