/* epic-cc#559: an i1 global through the PIC baseline backend. The shape is
 * what clang's own -O1 GlobalOpt produces for an internal flag only ever
 * written 0/1 (epic-cc#462): `@flag = global i1 false`, with the
 * representation inverted and the trailing test of it folded away.
 *
 * `out` receives the loaded flag byte, so the read is observable. The
 * backend used to panic on the i1 load with "only i8/i16 loads supported". */
volatile unsigned char out = 0;
static _Bool flag = 1;

void clear(void) { flag = 0; }

int main(void) {
    if (!flag) { out = 0; } else { out = 1; }
    clear();
    return 0;
}
