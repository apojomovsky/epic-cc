/* One PID update through the vendored epic-pid (epic-cc#840
 * scenario). Calls epic_pid_update from vendor/hal-pic18-base with
 * the marker protocol around it; the ladder case lists the vendored
 * pid.c plus the epic_math multiply it lowers to. PIC18 only: the
 * vendored fixed-point sources are the 18F4550 demo's. */
#include "pid.h"
#include <stdint.h>

volatile unsigned char bench_mark;
volatile int16_t setpoint = 1000;
volatile int16_t measurement = 960;
volatile int16_t out;

static epic_pid_t pid;

void main(void) {
    int16_t r;
    epic_pid_init(&pid, 256, 64, 32, -5000, 5000);
    bench_mark = 1;
    r = epic_pid_update(&pid, setpoint, measurement);
    out = r;
    bench_mark = 2;
}
