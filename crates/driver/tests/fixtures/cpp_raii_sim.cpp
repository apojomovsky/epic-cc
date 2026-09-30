#ifndef IN_VAL
#define IN_VAL 0
#endif

unsigned char g_log[4];
unsigned char g_n;

void note(unsigned char v) {
    if (g_n < 4) {
        g_log[g_n] = v;
        g_n = g_n + 1;
    }
}

struct G {
    unsigned char id;
    G(unsigned char i) : id(i) {}
    ~G() { note(id); }
};

unsigned char work(unsigned char x) {
    G g(1);
    if (x == 0) {
        return 10;
    }
    if (x == 1) {
        G h(2);
        return 20;
    }
    return 30;
}

volatile unsigned char out_vals[3];
volatile unsigned char out_n;

int main() {
    for (unsigned char x = 0; x < 3; x++) {
        out_vals[x] = work(x);
    }
    out_n = g_n;
    return 0;
}
