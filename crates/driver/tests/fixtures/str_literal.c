#include <stdint.h>
#include <string.h>

static volatile uint8_t g_tx[64];
static volatile uint8_t g_tx_len;

static void putc(char c) { g_tx[g_tx_len++] = (uint8_t)c; }

static void put_str(const char *s) {
    size_t n = strlen(s);
    for (size_t i = 0; i < n; i++) putc(s[i]);
}

int main(void) {
    put_str("epic-serial ready\r\n");
    return 0;
}
