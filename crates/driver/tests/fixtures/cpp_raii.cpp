unsigned char g_effect;

struct GuardNote {
    unsigned char id;
    GuardNote(unsigned char i) { id = i; }
    ~GuardNote() __attribute__((noinline));
};
GuardNote::~GuardNote() { g_effect = id; }

unsigned char work(unsigned char x) __attribute__((noinline));
unsigned char work(unsigned char x) {
    GuardNote g(1);
    if (x == 0) {
        return 10;
    }
    if (x == 1) {
        GuardNote h(2);
        return 20;
    }
    return 30;
}

volatile unsigned char in_val;
volatile unsigned char out_val;
volatile unsigned char out_fx;

int main() {
    out_val = work(in_val);
    out_fx = g_effect;
    return 0;
}
