void main(void) {
    unsigned char x = 1;
    asm volatile("movwf %0" : "+r"(x));
}
