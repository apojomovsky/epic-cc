#ifndef SEL
#define SEL 0
#endif

// Dispatch replays on hardware through the `tblrd-flash-ptr` superopt
// spec (#832); per-fixture runs stay sim-only (docs/40 §5).

class Base {
  public:
    unsigned char v;
    virtual unsigned char tick() { return v; }
};

class Derived : public Base {
  public:
    unsigned char tick() override { return v + 1; }
};

class More : public Base {
  public:
    unsigned char tick() override { return v + 2; }
};

Base g_b;
Derived g_d;
More g_m;

volatile unsigned char in_sel = SEL;
volatile unsigned char out_val;

int main() {
    Base* p = (in_sel == 0) ? (Base*)&g_b : (in_sel == 1) ? (Base*)&g_d : (Base*)&g_m;
    out_val = p->tick();
    return 0;
}
