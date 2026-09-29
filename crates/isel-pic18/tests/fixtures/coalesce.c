// Phi-edge coalescing (epic-cc#727): x is dead after the join, so the phi
// destination reuses its slot and isel skips the self-copy. The volatile
// probes keep both branch arms (and the phi) alive through -O1.
// sel = 1 -> m = x = arg + 1 = 42 -> out = 43.
// Straight-line range copies (epic-cc#739): w is dead after the trunc, so
// the trunc destination shares its slot base and isel skips the copy
// lane. The noinline boundaries keep a real call result and a real call
// argument through -O1, the menu-demo trunc pattern.
// tsrc = 0x1234 -> w = 0x1235 -> t = 0x35 -> tprobe = 0x35.
volatile unsigned char sel = 1;
volatile unsigned char arg = 41;
volatile unsigned char probe;
volatile unsigned char out;
volatile unsigned int tsrc = 0x1234;
volatile unsigned char tprobe;
__attribute__((noinline)) unsigned int getw(void) {
    return tsrc + 1;
}
__attribute__((noinline)) void take8(unsigned char v) {
    tprobe = v;
}
void main(void) {
    unsigned char x = arg + 1;
    unsigned char m;
    if (sel) {
        probe = 1;
        m = x;
    } else {
        probe = 2;
        m = 0;
    }
    out = m + 1;
    unsigned int w = getw();
    take8((unsigned char)w);
}
