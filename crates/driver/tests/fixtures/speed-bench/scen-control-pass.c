/* One control-loop pass through the vendored control demo
 * (epic-cc#873 scenario). Inits the real demo, injects a scripted ADC
 * reading via the sim override (so the pass never polls unmodeled ADC
 * hardware), then times a single control_demo_task_control pass with
 * the marker protocol around it: oversample, average, PID update, PWM
 * duty. Init only programs Timer2, never waits on it, and never
 * enables GIE, so the pass runs ISR-free with no timer model. */
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
