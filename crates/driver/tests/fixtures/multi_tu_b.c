static volatile unsigned char scratch;
__attribute__((noinline)) static unsigned char bump(unsigned char v) { scratch = v; return scratch + 2; }
unsigned char from_b(unsigned char v) { return bump(v); }   // 4 -> 6
