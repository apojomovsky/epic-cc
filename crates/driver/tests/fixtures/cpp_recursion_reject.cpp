volatile unsigned char sink;

unsigned char is_even(unsigned char n);

unsigned char is_odd(unsigned char n) {
    if (n == 0) {
        return 0;
    }
    return is_even(n - 1) + sink;
}

unsigned char is_even(unsigned char n) {
    if (n == 0) {
        return 1;
    }
    return is_odd(n - 1) + sink;
}

volatile unsigned char out_val;

int main() {
    out_val = is_odd(3);
    return 0;
}
