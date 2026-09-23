/* Task-table scan: struct array with non-power-of-two stride,
 * flag tests, and countdown arm. Menu-demo triage (epic-cc#624):
 * epic_taskmgr_tick (inlined into the overflow path, 166 vs XC8 15
 * plus an 88-word outlined tick) walks a 10-byte task table testing
 * flags and arming countdowns; task_stimulus walks a script table
 * the same way. The stride multiply per access is the shape. */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

#define MAX_TASKS 8

typedef struct {
    uint8_t flags;
    uint16_t period;
    uint16_t countdown;
    void *arg;
    uint8_t priority;
    uint8_t reserved[3];
} Task;

volatile Task g_tasks[MAX_TASKS];
volatile unsigned char fired;

static uint16_t arm_countdown(uint16_t period) {
    return (uint16_t)(period - 1U);
}

void main(void) {
    uint8_t i;
    for (i = 0; i < MAX_TASKS; i++) {
        Task *t = (Task *)&g_tasks[i];
        uint8_t f = t->flags;
        if ((f & 0x03U) == 0x03U) {
            if (t->countdown == 0U) {
                t->flags |= 0x04U;
                t->countdown = arm_countdown(t->period);
                fired++;
            } else {
                t->countdown--;
            }
        }
    }
}
