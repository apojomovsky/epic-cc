// epic-cc#1276: the guard emits `tail call void @llvm.assume(i1 %c)`
// (an `icmp ne i16` on the loaded bound), which legalize drops: an
// assumption carries no codegen. Expected: buf = {1, 2, 0, 0},
// tail = buf[2] = 0, out = buf[0] + buf[1] = 3.
int n = 1;
volatile unsigned char out;
unsigned char buf[4];
unsigned char tail;
void main(void) {
    __builtin_assume(n != 0);
    unsigned char *p = buf;
    for (int i = 0; i < 2; i++) {
        *p = (unsigned char)(i + 1);
        p++;
    }
    tail = *p;
    out = (unsigned char)(buf[0] + buf[1]);
}
