// P3 (PIC18) acceptance: a runtime RAM pointer, read and written through
// FSR0/INDF0, the pure pointer path, no const-flash involvement.
//
// This is PIC14's ptr_probe.c with the `const` table and its read
// REMOVED: on PIC18 a const-flash read needs TBLRD, which is P4's job
// (docs/29-pic18-port-design.md §4). See this plan's fixture-scope note
// for why the two backends' ptr_probe fixtures differ for now. Once P4
// lands, the ORIGINAL ptr_probe.c (unmodified) becomes a P4 acceptance
// addition for PIC18 too, and at that point running the same file through
// both backends is a clean parity check.
//
// `in` selects an index into `ram` (masked to 0-7, matching the array
// size); the value written is `in`'s low byte itself, then read back
// through the same pointer, so a wrong FSR/INDF sequence shows up as a
// wrong `out` rather than merely a crash.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char ram[8];
void main(void) {
    unsigned char i = (unsigned char)(in & 7);
    volatile unsigned char *p = ram + i;
    *p = (unsigned char)in;
    out = *p;
}
