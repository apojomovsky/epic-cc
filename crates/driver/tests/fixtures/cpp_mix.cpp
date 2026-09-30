struct Pair {
    unsigned char a;
    unsigned char b;
};

extern "C" unsigned char c_sum(const Pair* p);

volatile unsigned char out_sum;
volatile unsigned char out_first;

int main() {
    Pair p;
    p.a = 3;
    p.b = 4;
    out_sum = c_sum(&p);
    out_first = c_sum(&p) - p.b;
    return 0;
}
