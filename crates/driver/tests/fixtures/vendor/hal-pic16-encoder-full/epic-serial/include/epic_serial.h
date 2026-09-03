/*
 * Family-agnostic ring-buffered UART: RX always-on into a ring, TX
 * demand-driven (TXIE off until a write queues bytes). Installed via
 * USART handle callbacks, not the HAL's strong IRQHandler. Ring access
 * uses single-byte atomics (see ring-discipline note in the source).
 * Formatting via put_* (non-variadic); putch remains for XC8 printf.
 */

#ifndef EPIC_SERIAL_H
#define EPIC_SERIAL_H

#include <stdint.h>

/* Default ring-buffer size (power of two) for both TX and RX. Override by
 * defining EPIC_SERIAL_RING_SZ before including the header. */
#ifndef EPIC_SERIAL_RING_SZ
#define EPIC_SERIAL_RING_SZ 32u
#endif

/**
 * @brief Initialize the USART for async 8N1 at baud.
 *
 * Starts interrupt-driven RX (TX IT is armed on first write). Must be
 * called with interrupts enabled afterwards: init enables RCIE; TXIE is
 * enabled on demand by epic_serial_write.
 *
 * @param fosc_hz the system oscillator frequency in Hz
 * @param baud the desired baud rate
 */
void epic_serial_init(uint32_t fosc_hz, uint32_t baud);

/**
 * @brief Enqueue len bytes for background TX.
 *
 * Non-blocking: copies into the TX ring and enables the TX ISR to drain
 * it. If the ring fills, blocks until space frees (so the whole buffer
 * is sent).
 *
 * @param data bytes to transmit
 * @param len number of bytes to enqueue
 * @return the number of bytes enqueued (len unless len is 0)
 */
int epic_serial_write(const uint8_t *data, int len);

/**
 * @brief Pull up to max received bytes from the RX ring.
 *
 * Non-blocking.
 *
 * @param buf destination buffer
 * @param max maximum number of bytes to read
 * @return the number of bytes actually read (0 if nothing received)
 */
int epic_serial_read(uint8_t *buf, int max);

/**
 * @brief Report the number of bytes available to read from the RX ring.
 *
 * @return the number of received bytes buffered
 */
int epic_serial_available(void);

/**
 * @brief Report the number of bytes still pending in the TX ring.
 *
 * 0 means the ring is empty; the last byte may still be in the shift
 * register, use epic_serial_flush to wait for that.
 *
 * @return the number of bytes not yet loaded into TXREG
 */
int epic_serial_tx_pending(void);

/**
 * @brief Block until every enqueued TX byte has been transmitted.
 *
 * Waits for the TX ring to drain and the shift register to empty, so no
 * byte is mid-transmission on return. Use before sleep/reboot or to
 * pace output.
 */
void epic_serial_flush(void);

/**
 * @brief Emit one char through the TX ring (XC8 printf retarget).
 *
 * XC8's printf family calls putch per character, so defining this makes
 * printf stream over the UART on target. Host libc printf does not call
 * putch, so this is used directly only by target firmware.
 *
 * @param c the character to emit
 */
void putch(char c);

/**
 * @brief Emit one char through the TX ring.
 *
 * @param c the character to emit
 */
void epic_serial_put_char(char c);

/**
 * @brief Emit the NUL-terminated string s through the TX ring.
 *
 * @param s the string to emit (pass non-NULL)
 */
void epic_serial_put_str(const char *s);

/**
 * @brief Emit v in decimal with no leading zeros.
 *
 * @param v the value to emit
 */
void epic_serial_put_u16(uint16_t v);

/**
 * @brief Emit v in decimal with no leading zeros.
 *
 * @param v the value to emit
 */
void epic_serial_put_u32(uint32_t v);

/**
 * @brief Emit v in decimal, prefixed with - when negative.
 *
 * @param v the value to emit
 */
void epic_serial_put_i16(int16_t v);

/**
 * @brief Emit v in decimal, prefixed with - when negative.
 *
 * @param v the value to emit
 */
void epic_serial_put_i32(int32_t v);

/**
 * @brief Emit v as two uppercase hex digits.
 *
 * @param v the value to emit
 */
void epic_serial_put_hex8(uint8_t v);

/**
 * @brief Emit v as four uppercase hex digits.
 *
 * @param v the value to emit
 */
void epic_serial_put_hex16(uint16_t v);
/*
 * epic-cc has no stdio and cannot yet pass a const pointer across a
 * call boundary (epic-cc#148), so on that path put_str expands per
 * call site: the literal lands in const data and is copied byte-wise
 * into an exact-size RAM staging buffer, and the RAM copy is written
 * (the EPIC_HARNESS_LOG_STATIC pattern). The argument must be a string
 * literal; XC8 and host keep the out-of-line function.
 */
#ifdef __EPIC_CC__
#define epic_serial_put_str(s)                                                         \
    do {                                                                               \
        static const char epic_str_src_[] = s;                                         \
        static char epic_str_buf_[sizeof(epic_str_src_)];                              \
        uint8_t epic_str_i_;                                                           \
        typedef char epic_str_len_ok_[(sizeof(epic_str_src_) <= 256) ? 1 : -1];       \
        for (epic_str_i_ = 0u; epic_str_i_ < (uint8_t)sizeof(epic_str_src_);           \
             epic_str_i_++) {                                                          \
            epic_str_buf_[epic_str_i_] = epic_str_src_[epic_str_i_];                   \
        }                                                                              \
        epic_serial_write((const uint8_t *)epic_str_buf_,                              \
                          (int)sizeof(epic_str_src_) - 1);                             \
    } while (0)

/*
 * Literal-only printf for XC8-era call sites. A printf with arguments
 * is a compile error here (macro arity): the variadic ABI is
 * epic-cc#131 territory, so compose the put_* calls instead. Redefining
 * a stdio name is safe on this path because stdio.h does not exist.
 */
#define printf(str) epic_serial_put_str(str)
#endif /* __EPIC_CC__ */

#endif /* EPIC_SERIAL_H */
