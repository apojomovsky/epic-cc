// Phi-edge coalescing (epic-cc#727): x is dead after the join, so the phi
// destination reuses its slot and isel skips the self-copy. The volatile
// probes keep both branch arms (and the phi) alive through -O1.
// sel = 1 -> m = x = arg + 1 = 42 -> out = 43.
volatile unsigned char sel = 1;
volatile unsigned char arg = 41;
volatile unsigned char probe;
volatile unsigned char out;
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
}
