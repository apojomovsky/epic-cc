// Dispatch replays on hardware through the `tblrd-flash-ptr` superopt
// spec (#832); per-fixture runs stay sim-only (docs/40 §5).

#ifndef SEL
#define SEL 0
#endif

typedef unsigned char (*tick_fn)(unsigned char* self);
typedef struct {
    tick_fn tick;
    unsigned char v;
} Obj;

unsigned char base_tick(unsigned char* self) { return self[2]; }
unsigned char derived_tick(unsigned char* self) { return self[2] + 1; }
unsigned char more_tick(unsigned char* self) { return self[2] + 2; }

Obj g_b = { base_tick, 0 };
Obj g_d = { derived_tick, 0 };
Obj g_m = { more_tick, 0 };

volatile unsigned char in_sel = SEL;
volatile unsigned char out_val;

int main(void) {
    Obj* p = (in_sel == 0) ? &g_b : (in_sel == 1) ? &g_d : &g_m;
    out_val = p->tick((unsigned char*)p);
    return 0;
}
