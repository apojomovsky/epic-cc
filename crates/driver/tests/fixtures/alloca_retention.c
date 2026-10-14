/* A callee stores the address of the caller's local buffer in a global.
   `z` is defined after the call and the buffer is read back through the
   stored pointer while `z` is still live, so the buffer's slot must not
   be reused for `z` (epic-cc#1147). */
unsigned char in;
unsigned char out;
unsigned char out2;
unsigned char *volatile tab;

__attribute__((noinline)) void keep(unsigned char *p) { tab = p; }
__attribute__((noinline)) unsigned char mix(unsigned char a) { return a ^ 0x5A; }

void main(void) {
    unsigned char buf[2];
    buf[0] = in;
    keep(buf);
    unsigned char z = mix(in + 1);
    unsigned char r = tab[0];
    out = r;
    out2 = z;
}
