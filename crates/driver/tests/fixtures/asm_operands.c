volatile unsigned char t, y;
void main(void) {
    asm volatile("movf %1, w" : "+m"(t) : "m"(y));
}
