class Cfg {
  public:
    unsigned char v;
    Cfg();
};
Cfg::Cfg() { v = 5; }

unsigned char touch() {
    static Cfg s;
    return s.v;
}

volatile unsigned char out;

__attribute__((interrupt(0))) void isr(void) { out = touch(); }

int main() { return 0; }
