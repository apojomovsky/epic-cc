/**
 * Build-agnostic test/firmware harness: one example source builds for
 * the host simulator and a real XC8 target with no `#ifdef`; host is a
 * bounded program that pumps simulated time, target runs forever.
 */

#ifndef EPIC_HARNESS_H
#define EPIC_HARNESS_H

#include <stdint.h>

/**
 * @brief  Harness start-up. On the host this resets the simulated CPU and
 *         wires the sim IRQ callback to the family dispatcher; `cycles`
 *         bounds the upcoming run. On a real target this is a no-op
 *         (the CPU starts itself; `cycles` is ignored).
 * @param cycles bound on the host run: simulated instruction cycles to
 *               pump before the harness reports the run over; ignored
 *               on a real target.
 */
void epic_harness_init(uint32_t cycles);

/**
 * @brief  Advance simulated time by one instruction cycle. On the host
 *         this pumps the simulator; on a real target time advances on its
 *         own, so this is a no-op. Call it inside any loop that must let
 *         time pass (the idle loop, or a hardware-ready wait).
 */
void epic_harness_tick(void);

/**
 * @brief  Loop-continuation test. On the host returns 1 while the
 *         bounded run is in progress, 0 when it is over. On a real
 *         target always returns 1 (firmware runs forever).
 * @param iteration the current loop index; unused on a real target.
 * @return 1 while the run should continue, 0 when the host run is over.
 */
int epic_harness_running(uint32_t iteration);

/**
 * @brief  printf-style log line. On the host this prints to stdout; on a
 *         real target it is a no-op (no stdout), so examples can log
 *         unconditionally without dragging in <stdio.h> or #ifdef.
 * @param fmt printf-style format string; remaining arguments follow.
 */
void epic_harness_log(const char *fmt, ...);

/**
 * @brief  Map a pass/fail flag to a process exit code (0 = pass, 1 =
 *         fail), first emitting a fixed marker line through @ref
 *         epic_harness_log so any build's captured output (including a
 *         sim-target UART capture) has one reliable line to grep.
 * @param ok 1 for pass, 0 for fail.
 * @return 0 when `ok` is set, 1 otherwise.
 */
static inline int epic_harness_report(int ok)
{
#ifdef __EPIC_CC__
    /* The marker goes out as a RAM copy, never as a pointer into const
     * data: epic-cc reads a C-indexed const array through its table
     * readers but has no address form for const data used as a value. */
    static char line[27];
    static const char pass[] = "EPIC_HARNESS_RESULT: PASS\n";
    static const char fail[] = "EPIC_HARNESS_RESULT: FAIL\n";
    uint8_t i = 0;
    if (ok) {
        for (i = 0; i < (uint8_t)(sizeof(pass) - 1u); i++) line[i] = pass[i];
    } else {
        for (i = 0; i < (uint8_t)(sizeof(fail) - 1u); i++) line[i] = fail[i];
    }
    line[i] = '\0';
    epic_harness_log(line);
#else
    epic_harness_log(ok ? "EPIC_HARNESS_RESULT: PASS\n"
                        : "EPIC_HARNESS_RESULT: FAIL\n");
#endif
    return ok ? 0 : 1;
}

/**
 * @brief  Log a literal through a static RAM copy.
 *
 * Gate firmware never materializes a const address: epic-cc lowers a
 * C-indexed const array to its table readers but has no address form
 * for const data used as a value (epic-cc#138), so the literal is
 * declared locally and copied byte-wise into a RAM buffer before the
 * call. Every expansion site owns its buffers.
 */
#ifdef __EPIC_CC__
#define EPIC_HARNESS_LOG_STATIC(msg)                             \
    do {                                                         \
        static const char epic_log_src_[] = msg;                 \
        static char epic_log_buf_[sizeof(epic_log_src_)];        \
        uint8_t epic_log_i_;                                     \
        for (epic_log_i_ = 0;                                    \
             epic_log_i_ < (uint8_t)sizeof(epic_log_src_);       \
             epic_log_i_++) {                                    \
            epic_log_buf_[epic_log_i_] = epic_log_src_[epic_log_i_]; \
        }                                                        \
        epic_harness_log(epic_log_buf_);                         \
    } while (0)
#else
#define EPIC_HARNESS_LOG_STATIC(msg) epic_harness_log(msg)
#endif


/**
 * @brief  Fan out to every peripheral IRQHandler for the linked family.
 *         Each family implements this with the same name: it walks that
 *         family's interrupt sources and calls the matching
 *         `EPIC_*_IRQHandler` weak handler whose flag is set. The host
 *         harness registers it as the sim IRQ callback; the real target
 *         calls it from its interrupt vector. Declared here so the
 *         harness can name it without a family-specific include.
 */
void epic_dispatch_all_irqs(void);

#endif /* EPIC_HARNESS_H */
