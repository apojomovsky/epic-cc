/* Indirect read-modify-write through a 16-bit array: the scale-2 dynamic
 * index keeps the access off the `PLUSW` shape, so the const add stages
 * through `POSTINC0`/`INDF0`. The single-use add accumulates into the
 * load temp in place (epic-cc#969). Size-only pin; behavior is covered
 * by the isel-pic18 `indf_rmw` sim test. */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

volatile uint16_t g_ram[32];
volatile uint8_t g_idx;

void main(void) {
    uint8_t i = g_idx;
    g_ram[i] += 7u;
}
