/* Self-contained fixed-point PI step in Q8.8. Seeds stay positive so
 * no shift of a negative value is needed. The ladder's PID scenario
 * (scen-pid-update) runs the same math through the vendored epic-pid;
 * this kernel isolates the arithmetic from the call overhead. Plain
 * C99: the private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile int16_t setpoint = 1000;
volatile int16_t measurement = 960;
volatile int16_t kp_q8 = 256;
volatile int16_t ki_q8 = 64;
volatile int16_t integrator = 0;
volatile int16_t output;

void main(void) {
    int16_t err;
    int32_t p;
    int32_t q;
    int16_t u;
    bench_mark = 1;
    err = (int16_t)(setpoint - measurement);
    p = (int32_t)kp_q8 * err;
    q = (int32_t)ki_q8 * err;
    integrator = (int16_t)(integrator + (q >> 8));
    u = (int16_t)((p >> 8) + integrator);
    if (u > 5000) {
        u = 5000;
    }
    output = u;
    bench_mark = 2;
}
