/* Invariant struct fields reloaded across a call sequence.
 * Menu-demo triage (epic-cc#624): gpio4_send (546 vs XC8 291) reads
 * the same pin-map fields about twelve times through a context
 * pointer, once per WritePin call, instead of hoisting them into
 * temporaries; EPIC_USART_Init and menu_demo_task_ui reload handle
 * fields the same way. */
typedef unsigned char uint8_t;

typedef struct {
    uint8_t port;
    uint8_t pin;
} Pin;

typedef struct {
    Pin rs;
    Pin db4;
    Pin e;
} PinMap;

volatile PinMap g_pins;
volatile unsigned char g_in;
volatile unsigned char g_out;

static void write_pin(uint8_t port, uint8_t pin, uint8_t v) {
    if (v) {
        g_out = (uint8_t)(port + pin);
    } else {
        g_out = (uint8_t)(port - pin);
    }
}

static void send(PinMap *m, uint8_t byte) {
    write_pin(m->rs.port, m->rs.pin, (uint8_t)(byte & 0x01U));
    write_pin(m->db4.port, m->db4.pin, (uint8_t)(byte & 0x02U));
    write_pin(m->e.port, m->e.pin, 1U);
    write_pin(m->e.port, m->e.pin, 0U);
    write_pin(m->db4.port, m->db4.pin, (uint8_t)(byte & 0x04U));
    write_pin(m->rs.port, m->rs.pin, (uint8_t)(byte & 0x08U));
}

void main(void) {
    send((PinMap *)&g_pins, g_in);
}
