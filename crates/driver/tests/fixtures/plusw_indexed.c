/* Byte-indexed static array traffic for the `LFSR` + `PLUSW0`
 * lowering (epic-cc#665). The index rides in as an initializer via
 * IDX so `__start`'s zero-init cannot erase it (same trick as
 * named_struct_globals_e2e). Behavior pin; size is pinned by
 * bench-plusw in size-bench. */
typedef unsigned char uint8_t;

volatile uint8_t g_arr[16];
volatile uint8_t g_idx = IDX;
volatile uint8_t g_out0;
volatile uint8_t g_out1;
volatile uint8_t g_out2;
volatile uint8_t g_out3;

void main(void) {
    uint8_t k;
    for (k = 0; k < 16; k++) {
        g_arr[k] = (uint8_t)(k * 3u + 1u);
    }
    {
        uint8_t i = g_idx;
        uint8_t a = g_arr[i];
        uint8_t b = g_arr[i + 1];
        g_out0 = a;
        g_out1 = b;
        g_arr[i] = (uint8_t)(a + 1u);
        g_out2 = g_arr[i];
        g_out3 = g_arr[15 - i];
    }
}
