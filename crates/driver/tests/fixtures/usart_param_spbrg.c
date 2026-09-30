/* epic-cc#632: the parameterized USART_ComputeSPBRG callee. The body is
 * the HAL's, minus the register writes: the `(u32, u32, enum, enum, enum)`
 * prototype whose borrow chain used to lower to `SUBFWB` (W - f) where the
 * restoring divide needs `SUBWFB` (f - W). The driver folds nothing here,
 * because the caller forwards runtime fosc/baud. */

typedef enum { M_ASYNC = 0x0U, M_SYNC = 0x1U } Mode;
typedef enum { H_LOW = 0x0U, H_HIGH = 0x1U } Brgh;
typedef enum { G_8 = 0x0U, G_16 = 0x1U } Brg16;

unsigned short compute_spbrg(unsigned long fosc_hz, unsigned long baud,
                             Mode mode, Brgh brgh, Brg16 brg16)
{
    if (baud == 0UL)
    {
        return 0xFFFFU;
    }

    unsigned long divisor;
    if (mode == M_SYNC)
    {
        divisor = 4UL;
    }
    else if (brg16 == G_16)
    {
        divisor = (brgh == H_HIGH) ? 4UL : 16UL;
    }
    else
    {
        divisor = (brgh == H_HIGH) ? 16UL : 64UL;
    }

    unsigned long x = (fosc_hz / (divisor * baud)) - 1UL;
    unsigned long max = (brg16 == G_16) ? 65535UL : 255UL;
    if (x > max)
    {
        return 0xFFFFU;
    }
    return (unsigned short)x;
}
