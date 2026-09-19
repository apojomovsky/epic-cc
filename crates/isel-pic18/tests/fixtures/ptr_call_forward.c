// epic-cc#473: forwarding a runtime pointer VALUE as a call argument must
// copy its two bytes directly into the callee's param slot, not round-trip
// them through FSR0 (load the pointer into FSR0L/FSR0H, then immediately
// copy FSR0L/FSR0H back out) -- FSR0 is only actually needed where the
// pointer is dereferenced, inside `callee`.

typedef struct {
    unsigned char a;
} s_t;

__attribute__((noinline)) void callee(s_t *p) { p->a = 1; }
__attribute__((noinline)) void fwd(s_t *p) { callee(p); }

s_t *volatile vp; // set by the test to point at buf
s_t buf;

void main(void) { fwd(vp); }
