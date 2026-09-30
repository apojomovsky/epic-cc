/* epic-cc#632: driver for the parameterized USART_ComputeSPBRG path.
 *
 * `epic_serial_init` forwards its `fosc_hz`/`baud` parameters into the
 * inner call, so the divide stays runtime and the emitted borrow chain is
 * the one that regressed. Whole-program constant folding (which hid the
 * bug in the control-demo shape) cannot remove it here. */

typedef enum { M_ASYNC = 0x0U, M_SYNC = 0x1U } Mode;
typedef enum { H_LOW = 0x0U, H_HIGH = 0x1U } Brgh;
typedef enum { G_8 = 0x0U, G_16 = 0x1U } Brg16;

unsigned short compute_spbrg(unsigned long fosc_hz, unsigned long baud,
                             Mode mode, Brgh brgh, Brg16 brg16);

/* The real SPBRG:SPBRGH pair, so the mdb gate can read the result. */
#define SPBRG  (*(volatile unsigned char *)0xFAFu)
#define SPBRGH (*(volatile unsigned char *)0xFB0u)

/* Zero on entry (`__start` clears it) and volatile, so every constant below
 * is a runtime value no stage can fold. */
volatile unsigned long g_pad;
volatile unsigned short g_out;

void serial_init(unsigned long fosc_hz, unsigned long baud)
{
    unsigned short sp = compute_spbrg(fosc_hz, baud,
                                      M_ASYNC, H_HIGH, G_16);
    g_out = sp;
    SPBRG = (unsigned char)(sp & 0xFFu);
    SPBRGH = (unsigned char)(sp >> 8);
}

int main(void)
{
    /* 48 MHz, 9600 baud, async, BRGH high, BRG16 wide -> 1249 (0x04E1). */
    serial_init(g_pad + 48000000UL, g_pad + 9600UL);
    return 0;
}
