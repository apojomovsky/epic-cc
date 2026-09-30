// Read-after-write of a volatile register (epic-cc#812 probe B).
// A volatile read may differ from the last write (flags, timers,
// PORT vs LAT), so the load must re-read memory: forwarding the stored
// value drops observable behavior.
volatile unsigned char port_shadow;
volatile unsigned char flag;

void main(void) {
    port_shadow = 0x11;
    flag = port_shadow;
}
