/*
 * Interrupt-driven ring-buffered UART + printf retarget: RX is always-on
 * into a ring, TX is demand-driven (TXIE stays off until a write queues
 * bytes). Installed via the USART handle's RxCpltCallback/TxCpltCallback,
 * never by redefining the HAL's own strong USART_RX/TX_IRQHandler. Ring
 * access shared with an ISR relies on single-byte atomic fields (see the
 * ring-discipline note below), not critical sections.
 */

#include "epic_serial.h"
#include "epic_hal.h"               /* family umbrella: usart, sfr, irq, platform */
#include "core/epic_harness.h"      /* epic_dispatch_all_irqs (host TX drain)    */

#if defined(PIC18F2455) || defined(PIC18F2550) || defined(PIC18F4455) || defined(PIC18F4550)
  #define SERIAL_IS_PIC18     1
  #define SERIAL_IRQ_TX       PIC18_IRQ_USART_TX
  #define SERIAL_IRQ_RX       PIC18_IRQ_USART_RX
  #define SERIAL_TXREG_WRITE(b)  epic_sfr_write8(PIC_REG_TXREG, (uint8_t)(b))
#else
  #define SERIAL_IS_PIC18     0
  #define SERIAL_IRQ_TX       PIC16_IRQ_USART_TX
  #define SERIAL_IRQ_RX       PIC16_IRQ_USART_RX
  #define SERIAL_TXREG_WRITE(b)  (EPIC_REG8(PIC_REG_TXREG) = (uint8_t)(b))
#endif

#define SZ     EPIC_SERIAL_RING_SZ
#define MASK   (SZ - 1u)

static volatile uint8_t g_tx_buf[SZ], g_rx_buf[SZ];
static volatile uint8_t g_tx_head, g_tx_tail, g_tx_count;
static volatile uint8_t g_rx_head, g_rx_tail, g_rx_count;

/* Ring discipline (class-G): every field is one byte, so each
 * increment/decrement is a single atomic RMW on both families and
 * cannot tear. Producers write the buffer slot before bumping
 * index/count; consumers read the slot only after the count check, so
 * a half-written slot is never observed. The only ISR writer of the TX
 * ring is the USART TX handler (the RX handler touches only the RX
 * ring), so no GIE manipulation is needed: the Disable/Restore pairs
 * that used to guard these sites exposed the Finding 10.1 hazard (a
 * latched interrupt delivered inside a GIE=0 window tears the region
 * and can leave GIE cleared after ISR return). */

/* ISR callbacks (called by the HAL's USART handlers). */

/**
 * @brief Store one received byte into the RX ring.
 *
 * Called by the HAL's USART RX handler. Bytes are dropped on ring
 * overflow.
 *
 * @param data the received byte
 */
static void epic_serial_on_rx(uint8_t data)
{
    if (g_rx_count < SZ) {                  /* drop on overflow */
        g_rx_buf[g_rx_head] = data;
        g_rx_head = (uint8_t)((g_rx_head + 1u) & MASK);
        g_rx_count++;
    }
}

/**
 * @brief Load the next queued TX byte into TXREG.
 *
 * Called by the HAL's USART TX handler. Disarms the TX interrupt when
 * the ring is empty.
 */
static void epic_serial_on_tx(void)
{
    if (g_tx_count > 0u) {
        /* No GIE manipulation (see the ring-discipline note): the pop is
         * the only ISR writer of the TX ring's tail/count, and each
         * update is a single-byte atomic store. */
        uint8_t b = g_tx_buf[g_tx_tail];
        g_tx_tail = (uint8_t)((g_tx_tail + 1u) & MASK);
        g_tx_count--;
        SERIAL_TXREG_WRITE(b);              /* writing TXREG clears TXIF (HW) */
    } else {
        EPIC_IRQ_DisableSrc(SERIAL_IRQ_TX);  /* ring empty: disarm the TX ISR */
    }
}

/**
 * @brief Initialize the USART for async 8N1 at baud.
 *
 * Configures the family USART with the computed SPBRG value and installs
 * the RX/TX ISR callbacks; RX stays always-on and TX demand-driven.
 * See epic_serial.h for the full contract.
 *
 * @param fosc_hz the system oscillator frequency in Hz
 * @param baud the desired baud rate
 */
void epic_serial_init(uint32_t fosc_hz, uint32_t baud)
{
    /* Static: the USART driver stores the handle pointer for ISR use, so
     * a stack-local handle would dangle. */
    static USART_HandleTypeDef s_usart;
    USART_HandleTypeDef h = USART_HANDLE_DEFAULT;
    h.Mode      = USART_MODE_ASYNCHRONOUS;
    h.BaudHigh  = USART_BRGH_HIGH;
    h.DataWidth = USART_DATA_8BITS;
#if SERIAL_IS_PIC18
    h.BaudGen   = USART_BAUDGEN_16BIT;
    uint16_t sp = USART_ComputeSPBRG(fosc_hz, baud, USART_MODE_ASYNCHRONOUS,
                                     USART_BRGH_HIGH, USART_BAUDGEN_16BIT);
    h.SPBRG  = (uint8_t)(sp & 0xFFu);
    h.SPBRGH = (uint8_t)(sp >> 8);
#else
    uint16_t sp = USART_ComputeSPBRG(fosc_hz, baud, USART_MODE_ASYNCHRONOUS,
                                     USART_BRGH_HIGH);
    h.SPBRG  = (uint8_t)sp;
#endif
    /* Non-NULL callbacks make Init enable RCIE/TXIE + CREN/TXEN; RX stays
     * always-on and TX demand-driven, so TXIE goes off right after init. */
    h.RxCpltCallback = epic_serial_on_rx;
    h.TxCpltCallback = epic_serial_on_tx;
    s_usart = h;
    EPIC_USART_Init(&s_usart);
    EPIC_IRQ_DisableSrc(SERIAL_IRQ_TX);

    g_tx_head = g_tx_tail = g_tx_count = 0u;
    g_rx_head = g_rx_tail = g_rx_count = 0u;
}

/**
 * @brief Enqueue len bytes for background TX.
 *
 * Blocks (dispatching IRQs) while the TX ring is full so the whole
 * buffer is enqueued. See epic_serial.h for the full contract.
 *
 * @param data bytes to transmit
 * @param len number of bytes to enqueue
 * @return the number of bytes enqueued (len unless len is 0)
 */
int epic_serial_write(const uint8_t *data, int len)
{
    for (int i = 0; i < len; i++) {
        while (g_tx_count >= SZ) {
            epic_dispatch_all_irqs();        /* ring full: drain (host pumps, target ISR drains) */
        }
        g_tx_buf[g_tx_head] = data[i];
        g_tx_head = (uint8_t)((g_tx_head + 1u) & MASK);
        g_tx_count++;
        EPIC_IRQ_Enable(SERIAL_IRQ_TX);       /* kick the TX ISR */
    }
    return len;
}

/**
 * @brief Pull up to max received bytes from the RX ring.
 *
 * Non-blocking. See epic_serial.h for the full contract.
 *
 * @param buf destination buffer
 * @param max maximum number of bytes to read
 * @return the number of bytes actually read (0 if nothing received)
 */
int epic_serial_read(uint8_t *buf, int max)
{
    int n = 0;
    while (n < max && g_rx_count > 0u) {
        buf[n++] = g_rx_buf[g_rx_tail];
        g_rx_tail = (uint8_t)((g_rx_tail + 1u) & MASK);
        g_rx_count--;
    }
    return n;
}

/**
 * @brief Report the number of bytes available to read from the RX ring.
 *
 * @return the number of received bytes buffered
 */
int epic_serial_available(void)
{
    return (int)g_rx_count;                  /* single-byte read: atomic */
}

/**
 * @brief Report the number of bytes still pending in the TX ring.
 *
 * @return the number of bytes not yet loaded into TXREG
 */
int epic_serial_tx_pending(void)
{
    return (int)g_tx_count;
}

/**
 * @brief Block until every enqueued TX byte has been transmitted.
 *
 * Dispatches IRQs until the TX ring drains and the shift register
 * empties. See epic_serial.h for the full contract.
 */
void epic_serial_flush(void)
{
    while (g_tx_count > 0u) {
        epic_dispatch_all_irqs();
    }
    while (!EPIC_USART_IsTxShiftRegisterEmpty()) {
        epic_dispatch_all_irqs();            /* last byte still in the shift register */
    }
}

/**
 * @brief Emit one char through the TX ring (XC8 printf retarget).
 *
 * @param c the character to emit
 */
void putch(char c)
{
    uint8_t b = (uint8_t)c;
    epic_serial_write(&b, 1);
}

/* Formatting (the put_* family). On epic-cc a shared static buffer avoids
 * the [12 x i8] alloca that spikes irparse; on XC8 a stack buffer avoids
 * linking 12 bytes of static RAM into every consumer (e.g. pid). Same
 * decimal/hex semantics on both. */
#ifdef __EPIC_CC__
static char s_fmt_buf[12];               /* sign + 10 digits + NUL fits i32 */
/* epic_serial_put_str's shared staging buffer (epic-hal#123); external
 * linkage, every TU that expands the macro needs this symbol. */
char g_epic_serial_str_scratch[EPIC_SERIAL_STR_SCRATCH_SZ];
#endif
/**
 * @brief Emit v in decimal via a local buffer.
 * @param v the value to emit
 */
static void epic_serial_put_udec(uint32_t v)
{
#ifdef __EPIC_CC__
    uint8_t n = 0;
    do {
        s_fmt_buf[n] = (char)('0' + (int)(v % 10u));
        v /= 10u;
        n++;
    } while (v != 0u);
    while (n > 0u) {
        n--;
        epic_serial_write((const uint8_t *)&s_fmt_buf[n], 1);
    }
#else
    char buf[12];
    uint8_t n = 0;
    do {
        buf[n] = (char)('0' + (int)(v % 10u));
        v /= 10u;
        n++;
    } while (v != 0u);
    while (n > 0u) {
        n--;
        epic_serial_write((const uint8_t *)&buf[n], 1);
    }
#endif
}

/**
 * @brief Emit v in decimal with sign handling.
 * @param v Value to format.
 */
static void epic_serial_put_idec(int32_t v)
{
    if (v < 0) {
        uint8_t sign = (uint8_t)'-';
        /* -(-2147483648) overflows, so negate through the low half. */
        epic_serial_write(&sign, 1);
        epic_serial_put_udec((uint32_t)(-(v + 1)) + 1u);
    } else {
        epic_serial_put_udec((uint32_t)v);
    }
}

/**
 * @brief Emit the low nibbles of v as hex.
 * @param v Value to format.
 * @param nibbles Number of nibbles to emit.
 */
static void epic_serial_put_hexw(uint32_t v, int nibbles)
{
#ifdef __EPIC_CC__
    int base = (int)sizeof(s_fmt_buf) - nibbles;
    for (int i = nibbles - 1; i >= 0; i--) {
        int d = (int)(v & 0xFu);
        s_fmt_buf[base + i] = (char)((d < 10) ? ('0' + d) : ('A' - 10 + d));
        v >>= 4;
    }
    epic_serial_write((const uint8_t *)&s_fmt_buf[base], nibbles);
#else
    char buf[12];
    int base = (int)sizeof(buf) - nibbles;
    for (int i = nibbles - 1; i >= 0; i--) {
        int d = (int)(v & 0xFu);
        buf[base + i] = (char)((d < 10) ? ('0' + d) : ('A' - 10 + d));
        v >>= 4;
    }
    epic_serial_write((const uint8_t *)&buf[base], nibbles);
#endif
}

/**
 * @brief Emit one char through the TX ring.
 * @param c Char to emit.
 */
void epic_serial_put_char(char c)
{
    putch(c);
}

/* epic-cc expands put_str at each call site instead (see epic_serial.h):
 * a const pointer cannot cross a call boundary there yet (epic-cc#148). */
/**
 * @brief Emit a null-terminated string.
 * @param s Null-terminated string.
 */
#ifndef __EPIC_CC__
void epic_serial_put_str(const char *s)
{
    int len = 0;
    while (s[len] != '\0') {
        len++;
    }
    epic_serial_write((const uint8_t *)s, len);
}
#endif /* !__EPIC_CC__ */

/**
 * @brief Emit v in decimal.
 * @param v Value to format.
 */
void epic_serial_put_u16(uint16_t v)
{
    epic_serial_put_udec(v);
}

/**
 * @brief Emit v in decimal with no leading zeros.
 * @param v Value to format.
 */
void epic_serial_put_u32(uint32_t v)
{
    epic_serial_put_udec(v);
}

/**
 * @brief Emit v in decimal with sign.
 * @param v Value to format.
 */
void epic_serial_put_i16(int16_t v)
{
    epic_serial_put_idec(v);
}

/**
 * @brief Emit v in decimal with sign.
 * @param v Value to format.
 */
void epic_serial_put_i32(int32_t v)
{
    epic_serial_put_idec(v);
}

/**
 * @brief Emit v as two uppercase hex digits.
 * @param v Value to format.
 */
void epic_serial_put_hex8(uint8_t v)
{
    epic_serial_put_hexw(v, 2);
}

/**
 * @brief Emit v as four uppercase hex digits.
 * @param v Value to format.
 */
void epic_serial_put_hex16(uint16_t v)
{
    epic_serial_put_hexw(v, 4);
}
