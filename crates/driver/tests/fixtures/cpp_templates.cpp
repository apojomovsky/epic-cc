template <typename T>
class Box {
  public:
    T v;
    Box(T x) : v(x) {}
    T get() { return v; }
};

template <typename T>
T add2(T a, T b) {
    return a + b;
}

volatile unsigned char out_b8;
volatile unsigned char out_b16lo;
volatile unsigned char out_b16hi;
volatile unsigned char out_a8;
volatile unsigned char out_a16lo;
volatile unsigned char out_a16hi;

int main() {
    Box<unsigned char> b8(3);
    Box<unsigned int> b16(0x1234);
    out_b8 = b8.get();
    unsigned int w = b16.get();
    out_b16lo = w & 0xFF;
    out_b16hi = (w >> 8) & 0xFF;
    out_a8 = add2<unsigned char>(5, 6);
    unsigned int s = add2<unsigned int>(0x1000, 0x0020);
    out_a16lo = s & 0xFF;
    out_a16hi = (s >> 8) & 0xFF;
    return 0;
}
