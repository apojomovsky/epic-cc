/* Rewrite-A acceptance (epic-cc#639): an 8-byte aggregate of function
 * pointers with an escaping address. clang copies it as a wide
 * load/store pair, which the const-runs pass forwards into a memcpy so
 * the #504 counted TBLRD loop carries the LOW()/HIGH() bytes. */
typedef struct { void (*f0)(void); void (*f1)(void); void (*f2)(void); void (*f3)(void); } Ops;
typedef unsigned char (*probe_t)(const Ops *);
volatile unsigned char sink;
volatile unsigned char called;
static void fa(void) { called = 0xA1; }
static void fb(void) {}
static void fc(void) {}
static void fd(void) {}
static unsigned char probe(const Ops *p) { p->f0(); return called; }
volatile probe_t pfn;
void main(void) {
    Ops ops = { fa, fb, fc, fd };
    pfn = probe;
    sink = pfn(&ops);
}
