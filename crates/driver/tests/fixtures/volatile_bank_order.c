// Ordered volatile writes across banks (epic-cc#812 probe E).
// Volatile order holds across distinct addresses too (set-then-trigger
// sequencing), which is the PIC14 schedule pass's reorder trigger: a
// differently-banked write sandwiched between same-bank neighbors.
#ifdef _PIC18
#define SFR_LO (*(volatile unsigned char *)0xF81)
#define SFR_HI (*(volatile unsigned char *)0xF82)
#else
#define SFR_LO (*(volatile unsigned char *)0x05)
#define SFR_HI (*(volatile unsigned char *)0x85)
#endif

volatile unsigned char seq;

void main(void) {
    SFR_LO = 0x01;
    SFR_HI = 0x00;
    SFR_LO = 0x02;
    seq = SFR_LO;
}
