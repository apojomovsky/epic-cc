// epic-cc#1422: a flash (const) table address laundered through
// `uintptr_t` in value position (`(uintptr_t)tab`) and read back
// through an integer-to-pointer cast. The address passes through a
// volatile integer slot and a noinline identity call, so the cast
// cannot be folded away: the read must dispatch to flash (TBLRD) via
// the integer-domain space proof, not read RAM at the same address.
//
// Expected: out == 0x33 (tab[2]) and out2 == 0x11 (tab[0] through the
// call result).
typedef unsigned int uintptr_t;

static const unsigned char tab[4] = {0x11, 0x22, 0x33, 0x44};
volatile unsigned char out;
volatile unsigned char out2;
volatile uintptr_t laundered;

static uintptr_t __attribute__((noinline)) id(uintptr_t x) { return x; }

void main(void) {
    laundered = (uintptr_t)tab;
    out = *(const unsigned char *)(laundered + 2);
    out2 = *(const unsigned char *)id((uintptr_t)tab);
}
