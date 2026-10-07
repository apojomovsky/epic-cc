/* Sim-target implementation of the test harness (core/epic_harness.h):
 * real compiled firmware run under MPLAB SIM via mdb. Needs real USART
 * access for epic_harness_report's marker line; running() is bounded by
 * `cycles`; epic_harness_log writes fmt's raw bytes only (only the fixed
 * PASS/FAIL marker must be correct). */

#include "core/epic_harness.h"
#include "core/pic16_irq.h"
#include "peripherals/pic16f87xa_usart.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 20000000UL
#endif

#define EPIC_HARNESS_SIM_BAUD 9600UL

static uint32_t g_cycles = 0U;

/**
 * @brief TX-complete callback: non-null only to arm TXEN during init.
 */
static void s_tx_cplt(void)
{
    /* Non-null so EPIC_USART_Init arms TXEN (pic16f87xa_usart.c); never
     * actually called, transmission below is polled. */
}

/**
 * @brief Transmit one character, polling the TSR until it drains.
 * @param c the character to send.
 */
static void s_uart_putc(char c)
{
    while (!EPIC_USART_IsTxShiftRegisterEmpty())
    {
        /* wait for the shift register to drain */
    }
    EPIC_USART_Transmit((uint8_t)c);
}

/* Static, not a local: EPIC_USART_Init stores this pointer for the
 * ISR's whole lifetime (g_usart), so a local would dangle. Pinned to
 * bank 1 (0xA0) because the USART ISR's deref bakes a constant IRP=0
 * select (banks 0/1 only), the opposite window from the Timer0 ISR's
 * baked IRP=1. */
static USART_HandleTypeDef s_usart_handle EPIC_PLACE(0xA0);

/**
 * @brief Initialize the MPLAB SIM harness: configure the USART for
 *        polled output and record the cycle bound.
 * @param cycles the number of iterations the run is bounded by.
 */
void epic_harness_init(uint32_t cycles)
{
    g_cycles = cycles;

    s_usart_handle = (USART_HandleTypeDef)USART_HANDLE_DEFAULT;
    s_usart_handle.SPBRG = (uint8_t)USART_ComputeSPBRG(
        FOSC_HZ, EPIC_HARNESS_SIM_BAUD, USART_MODE_ASYNCHRONOUS,
        USART_BRGH_HIGH);
    s_usart_handle.TxCpltCallback = s_tx_cplt;
    (void)EPIC_USART_Init(&s_usart_handle);
    /* A non-null TxCpltCallback also enables TXIE. TXIF is pending
     * right after reset and only clears on a TXREG write, so once GIE
     * is enabled the TX ISR would fire forever. Transmission here is
     * polled, so turn the source back off after Init. */
    EPIC_IRQ_DisableSrc(PIC16_IRQ_USART_TX);
}

/**
 * @brief Advance time. Nothing to pump under MPLAB SIM: real time
 *        advances on its own.
 */
void epic_harness_tick(void)
{
    /* Real time advances on its own under MPLAB SIM too, nothing to pump. */
}

/**
 * @brief Report whether the run should continue.
 * @param iteration the current 0-based iteration index.
 * @return 1 while `iteration` is below the configured cycle bound, else 0.
 */
int epic_harness_running(uint32_t iteration)
{
    return (iteration < g_cycles) ? 1 : 0;
}

/**
 * @brief Write the format string's raw bytes to the USART (polled).
 * @param fmt the (already-formatted) string to send.
 */
void epic_harness_log(const char *fmt, ...)
{
    /* The polled transmit chain is 6 calls deep (report -> log -> putc
     * -> Transmit -> ClearFlag); the 1 ms tick ISR adds 4 more, which
     * overflows the 8-level hardware stack if it fires mid-print. Mask
     * GIE for the whole line so the marker cannot be corrupted. */
    uint8_t prev_gie = EPIC_IRQ_Disable();
    while (*fmt)
    {
        s_uart_putc(*fmt);
        fmt++;
    }
    EPIC_IRQ_Restore(prev_gie);
}
