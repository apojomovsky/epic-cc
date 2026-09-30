#ifndef IN_VAL
#define IN_VAL 0
#endif

volatile unsigned char in_val = IN_VAL;
volatile unsigned char out_val;

int main() {
    out_val = in_val + 1;
    return 0;
}
