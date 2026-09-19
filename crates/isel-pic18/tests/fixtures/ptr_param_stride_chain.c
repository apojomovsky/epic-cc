// epic-cc#469 review fix: a runtime POINTER PARAMETER (not a fixed
// global) indexed with a stride wide enough to trigger the shift-add
// chain must still fold the pointer's own value into the chain-scaled
// FSR0 pair. `emit_fsr0_indirect_slot`'s chain branch dropped this: it
// zero-seeded FSR0 and added scale*idx + the static field offset, never
// reading the pointer's own two bytes, so the write landed near address
// 0 instead of inside the pointed-to array.
//
// Two call sites with different pointer arguments (storage_a vs
// storage_b) keep the compiler from constant-folding the parameter back
// to a single known global, which would silently reroute this through
// the (already-correct) Absolute-origin path and defeat the point of
// the test.
struct Rec {
    unsigned short val;   // offset 0
    unsigned char tag;    // offset 2
    unsigned char pad[9]; // offset 3, pad to offset 12
};                        // sizeof == 12

volatile struct Rec storage_a[4];
volatile struct Rec storage_b[4];
volatile unsigned char out;
volatile unsigned char vi;
volatile unsigned char which;

__attribute__((noinline)) void touch(struct Rec *p, unsigned char idx) {
    p[idx].tag = 0x42;
}

void main(void) {
    vi = 2;
    which = 0;
    if (which) {
        touch(storage_a, vi);
    } else {
        touch(storage_b, vi);
    }
    out = storage_a[2].tag + storage_b[2].tag;
}
