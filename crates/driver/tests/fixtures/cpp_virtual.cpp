#ifndef SEL
#define SEL 0
#endif

class Base {
  public:
    unsigned char v;
    virtual unsigned char tick() { return v; }
};

class Derived : public Base {
  public:
    unsigned char tick() override { return v + 1; }
};

Base g_b;
Derived g_d;

volatile unsigned char in_sel = SEL;
volatile unsigned char out_val;

int main() {
    Base* p = (in_sel == 0) ? (Base*)&g_b : (Base*)&g_d;
    out_val = p->tick();
    return 0;
}
