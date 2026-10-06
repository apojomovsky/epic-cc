/* One control-loop pass through the vendored control demo
 * (epic-cc#873 scenario). Injects a scripted ADC reading via the sim
 * override (the sim models no ADC hardware), then marks around one
 * control_demo_task_control pass. The vendor oversample and average
 * helpers are __EPIC_CC__ stubs returning 0, so the pass pins the PID
 * update and PWM duty with a zero sample. Init never enables GIE, so
 * the pass runs ISR-free with no timer model. */
#include "control_demo_core.h"

#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint16_t out_measurement;
volatile int16_t out_output;

void main(void) {
    control_demo_init();
    control_demo_inject_adc(512);
    bench_mark = 1;
    control_demo_task_control((void *)0);
    out_measurement = control_demo_measurement();
    out_output = control_demo_output();
    bench_mark = 2;
}
