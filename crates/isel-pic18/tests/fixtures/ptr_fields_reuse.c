// epic-cc#472: adjacent field accesses through the same, unchanged runtime
// pointer must reuse FSR0's tracked position (a forward delta add) instead
// of re-deriving the whole address (base reload + absolute offset) from
// scratch for every field.

typedef struct {
    unsigned char a;
    unsigned char b;
    unsigned char c;
    unsigned char d;
} cfg_t;

cfg_t buf;
cfg_t *volatile vp = &buf; // points at buf

void main(void) {
    cfg_t *p = vp;
    p->a = 1;
    p->b = 2;
    p->c = 3;
    p->d = 4;
}
