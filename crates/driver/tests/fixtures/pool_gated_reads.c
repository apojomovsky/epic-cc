// Pooled member reads at a constant offset (epic-cc#913): with `--const-pool`
// the memcpy source and the string pointer resolve to members of
// `__const_pool`, and their reads go through the chunk reader at the member's
// offset. The outputs must match the unpooled build.
#include <stdint.h>
#include <string.h>

static volatile uint8_t g_out0;
static volatile uint8_t g_out1;
static volatile uint8_t g_out2;

static const uint8_t tbl[8] = {10, 20, 30, 40, 50, 60, 70, 80};
static const char txt[] = "hello-world";

static uint8_t pick(const char *p) {
    return (uint8_t)p[2];
}

int main(void) {
    uint8_t buf[4];
    memcpy(buf, &tbl[2], 4);
    g_out0 = (uint8_t)(buf[0] + buf[3]);
    g_out1 = pick(txt + 1);
    g_out2 = (uint8_t)strlen(txt);
    return 0;
}
