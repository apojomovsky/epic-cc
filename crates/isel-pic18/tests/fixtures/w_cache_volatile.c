// epic-cc#502: two volatile reads of one global with a store between them.
// The global's byte must come from the file register every time, since an
// interrupt can change it between accesses.
volatile unsigned char in;
volatile unsigned char slot;
volatile unsigned char out;
void main(void) {
    slot = in;
    out = in;
}
