class V {
  public:
    unsigned char x;
    V(unsigned char v) : x(v) {}
    V operator+(const V& o) { return V(x + o.x); }
    unsigned char operator==(const V& o) { return x == o.x ? 1 : 0; }
};

volatile unsigned char out_sum;
volatile unsigned char out_eq;
volatile unsigned char out_ne;

int main() {
    V a(3), b(4);
    V c = a + b;
    out_sum = c.x;
    out_eq = (a == b);
    out_ne = (a == a);
    return 0;
}
