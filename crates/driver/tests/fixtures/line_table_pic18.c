volatile unsigned char a;
volatile unsigned char b;
volatile unsigned char sink;
__attribute__((noinline)) void bump(void) {
    a = 1;
    b = a;
}
void main(void) {
    bump();
    sink = b;
    bump();
    sink = a;
}
