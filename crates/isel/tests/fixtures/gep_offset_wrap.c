// Direct load and store through a GEP whose total offset passes 255:
// the static part is 200 or 100 and the runtime term adds 200 more, so
// the sums are 400 (store) and 300 (load). The anchor array is small;
// both accesses land past it in open RAM, and the sim checks the byte
// at the true address, not the low-byte wrap (epic-cc#1466).
// Expected for in == 200: the store puts 0xA5 at big + 400 and
// out == 0x3C, the byte seeded at big + 300.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char big[16];

void main(void) {
    unsigned char i = (unsigned char)in;
    big[200 + i] = 0xA5;
    out = big[100 + i];
}
