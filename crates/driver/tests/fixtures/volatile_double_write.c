// Back-to-back volatile writes to one address (epic-cc#812 probe A).
// Models the EECON2 0x55/0xAA unlock: the first store looks dead to a
// dead-store or store-merge pass but must survive, on a global and on an
// SFR (the literal-pointer path, one instruction per write).
#ifdef _PIC18
#define SFR (*(volatile unsigned char *)0xF81)
#else
#define SFR (*(volatile unsigned char *)0x06)
#endif

volatile unsigned char ee;

void main(void) {
    ee = 0x55;
    ee = 0xAA;
    SFR = 0x55;
    SFR = 0xAA;
}
