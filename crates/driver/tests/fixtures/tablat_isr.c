// epic-cc#532 regression: an ISR that performs a const (flash) read must
// not corrupt the preempted main context's in-flight TBLRD value.
//
// A const read is `TBLRD*` followed by a consumer that moves TABLAT
// (0xFF5), so the flash byte is live in TABLAT across one instruction
// boundary. ADR-013 saves TBLPTR (the address half of that sequence) but
// TABLAT (the value half) was outside every save set, so a handler whose
// own const read emits TBLRD resumed main against the handler's byte.
//
// Both indices are volatile so neither read folds to a MOVLW constant.
const unsigned char tbl[8] = {0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88};

volatile unsigned char idx = 1;      /* main reads tbl[1] == 0x22 */
volatile unsigned char out;

volatile unsigned char isr_idx = 7;  /* handler reads tbl[7] == 0x88 */
volatile unsigned char isr_out;

__attribute__((interrupt(0))) void isr(void) {
    isr_out = tbl[isr_idx];
}

int main(void) {
    out = tbl[idx];
    return 0;
}
