#ifndef IN_VAL
#define IN_VAL 0
#endif

class Acc {
  public:
    unsigned char total;
    void add(unsigned char x) { total = total + x; }
    unsigned char get() { return total; }
};

namespace cfg {
class Scale {
  public:
    unsigned char factor;
    unsigned char apply(unsigned char x) { return x + factor; }
};
}

Acc g_acc;
cfg::Scale g_scale;
volatile unsigned char in_val = IN_VAL;
volatile unsigned char out_val;

int main() {
    g_acc.add(in_val);
    g_scale.factor = 2;
    out_val = g_scale.apply(g_acc.get());
    return 0;
}
