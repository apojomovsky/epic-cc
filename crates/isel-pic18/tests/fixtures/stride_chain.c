// Shift-add index-scaling acceptance: struct arrays with a 12-byte
// element stride, big enough that the backend scales the runtime index
// with a shift-add chain instead of unrolled per-byte adds on the
// FSR0/INDF0 path (the volatile RAM array); the flash const array takes
// the same chain shape on the TBLPTR triple.
// A wrong chain lands every access on the wrong element, so the sim
// assertion catches it.
//
// Expected: out == 0x4E; the hand trace lives in the e2e test.
struct Rec {
    unsigned short val;      // offset 0
    unsigned char tag;       // offset 2
    unsigned char pad[9];    // offset 3, pad to offset 12
};                           // sizeof == 12

volatile struct Rec recs[6];
const struct Rec crecs[3] = {
    {0x0102, 1, {0}},
    {0x0304, 2, {0}},
    {0x0605, 3, {0}},
};
volatile unsigned char out;
volatile unsigned char vi;
volatile unsigned char vj;

void main(void) {
    vi = 3;
    vj = 2;
    recs[vi].val = 0x1234;                  // RAM store through a chain-scaled FSR0
    recs[5].tag = 7;                        // constant index must not disturb the chain path
    out = recs[vi].tag;                     // 0
    out += (unsigned char)recs[vi].val;     // + 0x34
    out += (unsigned char)(recs[vi].val >> 8); // + 0x12
    out += crecs[vj].tag;                   // + 3 (TBLPTR chain-scaled const read)
    out += (unsigned char)crecs[vj].val;    // + 0x05
}
