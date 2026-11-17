// epic-cc#1417: a for-loop's induction variable can start from a stale
// overlay slot.
//
// The loop header is the target of a `switch` case, so the induction
// variable's phi reaches the header through the switch's compare
// chain, not through the switch block itself. The counter's slot is
// shared with the high byte of the request's length, which the switch
// chain writes before the loop is entered, so a backend that drops the
// counter's zero init counts from that stale byte: the loop copies
// nothing and indexes far outside the array.

#include <stdint.h>
#include <epic-cc.h>

static volatile uint8_t ep0_out[8] EPIC_AT(0x500);
static volatile uint8_t ep0_in[8] EPIC_AT(0x508);
static volatile uint8_t g_line_coding[7] EPIC_AT(0x040);

volatile uint8_t r;

static void handle_setup(void)
{
    uint8_t req = ep0_out[1];
    uint16_t value = (uint16_t)ep0_out[2] | ((uint16_t)ep0_out[3] << 8);
    uint16_t length = (uint16_t)ep0_out[6] | ((uint16_t)ep0_out[7] << 8);

    switch (req) {
    case 0x20:
        r = 0x11;
        break;
    case 0x21: {
        uint8_t i;
        for (i = 0; i < 7; i++) {
            ep0_in[i] = g_line_coding[i];
        }
        break;
    }
    case 0x22:
        r = 0x22;
        break;
    default:
        r = (uint8_t)(value + length);
        break;
    }
}

int main(void)
{
    uint8_t i;
    for (i = 0; i < 8; i++) {
        g_line_coding[i] = (uint8_t)(0xA0 + i);
    }
    ep0_out[0] = 0xA1;
    ep0_out[1] = 0x21;
    ep0_out[7] = 0xA1;
    handle_setup();
    r = ep0_in[0];
    return 0;
}
