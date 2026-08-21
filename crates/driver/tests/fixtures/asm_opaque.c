volatile unsigned char counter, flag;
void main(void) {
    asm volatile("bcf INTCON, 7");
    counter = counter + 1;
    asm volatile("bsf INTCON, 7");
    flag = 1;
}
