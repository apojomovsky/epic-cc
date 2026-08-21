static volatile unsigned char scratch;
__attribute__((noinline)) static unsigned char bump(unsigned char v) { scratch = v; return scratch + 1; }
unsigned char from_a(unsigned char v) { return bump(v); }   // 3 -> 4
