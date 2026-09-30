#ifndef IN_VAL
#define IN_VAL 0
#endif

class Cfg {
  public:
    unsigned char v;
    Cfg();
    ~Cfg();
};
Cfg::Cfg() { v = 9; }
Cfg::~Cfg() { v = 0; }
Cfg g_cfg;

unsigned char use_local() {
    static Cfg s;
    return s.v;
}

volatile unsigned char in_val = IN_VAL;
volatile unsigned char out_global;
volatile unsigned char out_local;

int main() {
    out_global = g_cfg.v + in_val;
    out_local = use_local();
    return 0;
}
