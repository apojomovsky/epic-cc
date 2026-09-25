/* Byte-indexed static arrays: PIC18 `LFSR` + `PLUSW0` shapes.
 * Menu-demo triage (epic-cc#665): array and pointer access lowers
 * through explicit 16-bit FSR arithmetic around `INDF0`, where a
 * byte-indexed static array wants one `LFSR` plus `MOVF i,W` +
 * `MOVF PLUSW0,W` per access, with the pointer resident across
 * consecutive accesses. Size-only pin; behavior is covered by
 * plusw_indexed_e2e. */
typedef unsigned char uint8_t;

volatile uint8_t g_arr[16];
volatile uint8_t g_idx;
volatile uint8_t g_out;

void main(void) {
    uint8_t i = g_idx;
    g_out = g_arr[i];
    g_out = g_arr[i + 1];
    g_arr[i] = (uint8_t)(g_out + 1u);
    g_out = g_arr[i + 2];
}
