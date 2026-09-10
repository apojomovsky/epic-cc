// Regression (epic-cc#325 follow-up): a store of a BANK-1 value through a
// genuinely runtime indirect pointer to a BANK-0 destination. The value
// load (MOVF src) emits a BSF FSR,5 reassert that runs AFTER the FSR
// setup for the pointer (bank 0), clobbering it before the MOVWF INDF:
// without staging the value in common RAM first, the store lands in bank
// 1 and buf[0] keeps its preload. Expected: buf[0] = 5, src = 5.
volatile unsigned char buf[16];  // bank 0, 0x10-0x1F
volatile unsigned char src;      // bank 1, 0x30
volatile unsigned char *volatile p;
__attribute__((noinline)) void store(volatile unsigned char *q) {
    *q = src;
}
void main(void) {
    buf[0] = 9;
    src = 5;
    p = &buf[0];
    store(p);
}
