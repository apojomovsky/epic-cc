/* Handle-default plus field overrides plus by-pointer init call.
 * Menu-demo triage (epic-cc#617): menu_demo_init's cluster (init of the
 * ADC/TIMER2/CCP handle structs) is 2171 words. The shape is
 * a large default-initialized struct, scalar field overrides, and one
 * by-pointer init call, repeated per peripheral. */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

typedef struct {
    uint8_t Instance;
    uint8_t Mode;
    uint16_t CompareValue;
    uint8_t Period;
    uint8_t Duty;
    uint8_t OutputMode;
    uint8_t DeadBandDelay;
    uint8_t AutoRestart;
    uint8_t ShutdownSource;
    uint8_t ShutdownPins;
    void (*EventCallback)(void);
    uint8_t Reserved[8];
} Handle;

#define HANDLE_DEFAULT { 1, 2, 0, 255, 0, 1, 0, 1, 0, 0, 0, {0} }

volatile unsigned char in;
volatile unsigned char sink;

static void peripheral_init(const Handle *h) {
    sink = h->Period ^ h->Duty ^ h->Mode ^ in;
}

void main(void) {
    Handle h = HANDLE_DEFAULT;
    h.Mode = 3;
    h.Duty = in;
    h.DeadBandDelay = 5;
    h.ShutdownSource = 1;
    peripheral_init(&h);
}
