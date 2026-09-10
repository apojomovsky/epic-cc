// P4 stack-budget regression: a const read inside a callee. The reader
// CALL nests `__start -> main -> at` + `__read` (3 levels) on the 2-level
// silicon stack, whose shift register drops the oldest return with no
// trap. Compilation must panic loudly instead of emitting it. `noinline`
// keeps the callee (and the depth-2 read) from being folded away.
volatile unsigned short in;
volatile unsigned char out;
const unsigned char t[8] = {0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88};
__attribute__((noinline)) unsigned char at(unsigned char i) {
    return t[i];
}
void main(void) {
    unsigned char i = (unsigned char)(in & 7);
    out = at(i);
}
