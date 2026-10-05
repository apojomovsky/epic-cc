/* Two u32 decimal-digit loops for the __udec_u32 helper test (epic-cc#722).
 * Same shape as the serial put_udec loop: do-while over udiv-10 with each
 * digit stored to a plain global buffer, so legalize shares both loops as
 * helper calls (sharing needs at least two sites; one lone loop stays
 * expanded). `in` is volatile (seeded after the start clear, like
 * bench-u16-dec); the buffers stay plain, matching s_fmt_buf. */
typedef unsigned long uint32_t;
typedef unsigned char uint8_t;

volatile uint32_t in;
unsigned char buf[12];
unsigned char buf2[12];
unsigned char n_out;
unsigned char n2_out;

void main(void) {
    uint32_t v = in;
    uint8_t n = 0;
    do {
        buf[n] = (char)('0' + (int)(v % 10u));
        v /= 10u;
        n++;
    } while (v != 0u);
    n_out = n;
    v = in;
    n = 0;
    do {
        buf2[n] = (char)('0' + (int)(v % 10u));
        v /= 10u;
        n++;
    } while (v != 0u);
    n2_out = n;
}
