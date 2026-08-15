volatile unsigned char in;
volatile unsigned char out;
__attribute__((noinline)) static int add(int a, int b) { return a + b; }
void main(void) {
    int n = in;
    int t = 0;
    for (int i = 0; i < n; i++) {
        if (i & 1) t = add(t, i);
        else      t = add(t, 100);
    }
    out = (unsigned char)t;
}
