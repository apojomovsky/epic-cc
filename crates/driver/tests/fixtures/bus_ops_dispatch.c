/* epic-hal epic-bus shape (epic-cc#1223): ops tables of function
 * pointers installed through a RAM pointer global (a store of a const
 * address) and dispatched field by field from mem transactions. */
#include <stdint.h>

typedef struct {
    void    (*start)(void);
    void    (*repeated_start)(void);
    void    (*stop)(void);
    int     (*write_byte)(uint8_t b);
    uint8_t (*read_byte)(int ack);
} ops_t;

volatile uint8_t g_out;
static uint8_t g_log[8];
static uint8_t g_n;

static void m_start(void) { g_log[g_n++ & 7] = 1; }
static void m_rstart(void) { g_log[g_n++ & 7] = 2; }
static void m_stop(void) { g_log[g_n++ & 7] = 3; }
static int m_write(uint8_t b) { g_log[g_n++ & 7] = b; return 1; }
static uint8_t m_read(int ack) { return (uint8_t)(0x40 + ack); }

static void d_start(void) { }
static int d_write(uint8_t b) { (void)b; return 0; }
static uint8_t d_read(int ack) { (void)ack; return 0; }

static const ops_t g_mock = { m_start, m_rstart, m_stop, m_write, m_read };
static const ops_t g_default = { d_start, d_start, d_start, d_write, d_read };
static const ops_t *g_ops = &g_default;

__attribute__((noinline)) void set_ops(const ops_t *ops)
{
    g_ops = ops ? ops : &g_default;
}

__attribute__((noinline)) void reset_ops(void) { g_ops = &g_default; }

__attribute__((noinline)) int mem_write(uint8_t dev, uint8_t reg, const uint8_t *data, int n)
{
    const ops_t *o = g_ops;
    o->start();
    if (!o->write_byte((uint8_t)(dev << 1))) { o->stop(); return -1; }
    if (!o->write_byte(reg)) { o->stop(); return -1; }
    for (int i = 0; i < n; i++)
        if (!o->write_byte(data[i])) { o->stop(); return -1; }
    o->stop();
    return n;
}

__attribute__((noinline)) int mem_read(uint8_t dev, uint8_t reg, uint8_t *buf, int n)
{
    const ops_t *o = g_ops;
    o->start();
    if (!o->write_byte((uint8_t)(dev << 1))) { o->stop(); return -1; }
    if (!o->write_byte(reg)) { o->stop(); return -1; }
    o->repeated_start();
    for (int i = 0; i < n; i++)
        buf[i] = o->read_byte(i < (n - 1) ? 1 : 0);
    o->stop();
    return n;
}

int main(void)
{
    static const uint8_t pat[2] = { 0x5A, 0xA5 };
    uint8_t rd[2] = { 0, 0 };
    reset_ops();
    set_ops(&g_mock);
    int w = mem_write(0x50, 0x10, pat, 2);
    int r = mem_read(0x50, 0x10, rd, 2);
    g_out = (uint8_t)(w == 2 && r == 2 && rd[0] == 0x41 && rd[1] == 0x40 && g_n == 11);
    for (;;) { }
}
