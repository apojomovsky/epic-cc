/*
 * PIC18F2455-family sim-target implementation of the test harness (see
 * core/epic_harness.h): runs as real compiled firmware under MPLAB SIM,
 * driven headlessly via mdb. Mirrors `pic16_harness_mdb.c`; only
 * the EUSART surface differs (`USART_ComputeSPBRG` takes an extra
 * BRG-width argument here).
 */

#include "core/epic_harness.h"
#include "core/pic18_irq.h"
#include "peripherals/pic18fxx5x_usart.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 48000000UL
#endif

#define EPIC_HARNESS_SIM_BAUD 9600UL

static uint32_t g_cycles = 0U;

/**
 * @brief  Transmit one character on the EUSART, blocking until the shift
 *         register drains.
 * @param c character to transmit.
 */
static void s_uart_putc(char c)
{
    while (!EPIC_USART_IsTxShiftRegisterEmpty())
    {
        /* wait for the shift register to drain */
    }
    EPIC_USART_Transmit((uint8_t)c);
}

/**
 * @brief  Harness start-up (sim-target build): configures the EUSART for
 *         polled 9600-baud output and stores `cycles` as the run bound.
 *         The TX interrupt source stays disarmed throughout (TXEN without
 *         TXIE); transmission here is polled.
 * @param cycles bound on the run; unused on the real target.
 */
void epic_harness_init(uint32_t cycles)
{
    g_cycles = cycles;

    /* BRGH=LOW (divisor 64) at this file's 48 MHz FOSC_HZ needs SPBRG=77
     * for 9600 baud, comfortably in the 8-bit BRG's range; BRGH=HIGH
     * (divisor 16) would need SPBRG=311, which doesn't fit. */
    USART_HandleTypeDef h = USART_HANDLE_DEFAULT;
    h.BaudHigh = USART_BRGH_LOW;
    h.SPBRG = (uint8_t)USART_ComputeSPBRG(FOSC_HZ, EPIC_HARNESS_SIM_BAUD,
                                           USART_MODE_ASYNCHRONOUS,
                                           USART_BRGH_LOW,
                                           USART_BAUDGEN_8BIT);
    h.TxCpltCallback = NULL;
    /* Polled transmission: a null callback leaves the TX source
     * disabled in init. Set TXEN explicitly afterwards: arming TXIE
     * for polled TX latches the level-held TXIF into a vector storm
     * no flag-clear can quiet (epic-hal#270). Split read+write, XC8
     * cannot lower a compound assignment on a volatile lvalue. */
    (void)EPIC_USART_Init(&h);
    uint8_t txsta = epic_sfr_read8(PIC_REG_TXSTA);
    txsta |= PIC_TXSTA_TXEN;
    epic_sfr_write8(PIC_REG_TXSTA, txsta);
}

/**
 * @brief  Advance simulated time by one instruction cycle (sim-target
 *         build): real time advances on its own under MPLAB SIM, so this
 *         is a no-op.
 */
void epic_harness_tick(void)
{
    /* Real time advances on its own under MPLAB SIM too, nothing to pump. */
}

/**
 * @brief  Loop-continuation test (sim-target build): returns 1 while the
 *         bounded run is in progress, 0 when it is over.
 * @param iteration the current loop index.
 * @return 1 while the run should continue, 0 when the host run is over.
 */
int epic_harness_running(uint32_t iteration)
{
    return (iteration < g_cycles) ? 1 : 0;
}

/**
 * @brief  printf-style log line (sim-target build): transmits the format
 *         string over the EUSART, one character at a time.
 * @param fmt printf-style format string.
 */
void epic_harness_log(const char *fmt, ...)
{
    while (*fmt)
    {
        s_uart_putc(*fmt);
        fmt++;
    }
}
