// Cross-context dispatch shape (epic-hal#86 / epic-cc#152 follow-up):
// a task table whose fn field is stored by main while the ISR reads
// only the flags/countdown fields. The ISR's tick must not pull the
// stored tasks into the ISR context (field-sensitive ISR-read
// tracking), or the main-context dispatch loses its candidates and
// traps. A 1-arg i8 callback site (RB change) must NOT collect the
// 1-arg ptr-param tasks (candidate width filter).

#include <stdint.h>

typedef void (*task_fn_t)(void *arg);

struct tcb {
    task_fn_t fn;
    uint16_t period;
    uint16_t countdown;
    uint8_t flags;
};
static struct tcb g_tasks[2];

volatile uint8_t g_seen;   /* task effect (the arg's value) */
volatile uint8_t g_rb;     /* RB callback effect */
static void (*g_rb_cb)(uint8_t);

static void my_task(void *arg) { g_seen = *(uint8_t *)arg; }

void ISR_Tick(void) {
    /* ISR context: reads only flags/countdown of the task table. */
    for (uint8_t i = 0; i < 2; i++) {
        if (g_tasks[i].flags & 1u) {
            g_tasks[i].countdown--;
        }
    }
    /* The RB-style 1-arg i8 callback site. */
    if (g_rb_cb) g_rb_cb(0xAB);
}

static void on_rb(uint8_t v) { g_rb = v; }

volatile uint8_t g_arg_val;

static void run_ready(void) {
    for (uint8_t i = 0; i < 2; i++) {
        if (g_tasks[i].flags & 1u) {
            g_tasks[i].flags &= 0xFE;
            g_tasks[i].fn(&g_arg_val);
        }
    }
}

int main(void) {
    g_arg_val = 42;
    g_tasks[0].fn = my_task;
    g_tasks[0].flags = 1;
    g_rb_cb = on_rb;
    run_ready();
    ISR_Tick();
    return 0;
}
