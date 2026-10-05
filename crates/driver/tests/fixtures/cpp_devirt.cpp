// Dispatch replays on hardware through the `tblrd-flash-ptr` superopt
// spec (#832); per-fixture runs stay sim-only (docs/40 §5).

class Base {
  public:
    unsigned char v;
    virtual unsigned char tick() { return v; }
};

class Derived : public Base {
  public:
    unsigned char tick() override { return v + 7; }
};

Derived g_d;

volatile unsigned char out_val;

int main() {
    Base* p = &g_d;
    out_val = p->tick();
    return 0;
}
