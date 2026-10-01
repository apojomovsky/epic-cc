/**
 * MCP23017 (I2C) / MCP23S17 (SPI) 16-bit remote I/O expander driver
 * (Microchip DS20001952) on the HAL via epic-bus. One family-agnostic
 * driver covers both parts: the register map is identical, only the
 * transport differs. All accessors return 0 on success or -1 when the
 * transport reports a NACK/error, so a missing device is surfaced,
 * never swallowed.
 */

#ifndef EPIC_MCP23X17_H
#define EPIC_MCP23X17_H

#include <stddef.h>   /* NULL */
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Which 8-bit port a register access targets. */
typedef enum {
    EPIC_MCP23X17_PORTA = 0,
    EPIC_MCP23X17_PORTB = 1,
} epic_mcp23x17_port_t;

/** Which serial bus the part hangs on. */
typedef enum {
    EPIC_MCP23X17_BUS_I2C = 0,   /**< MCP23017 */
    EPIC_MCP23X17_BUS_SPI = 1,   /**< MCP23S17 */
} epic_mcp23x17_bus_t;

/**
 * Injectable register transport. When a handle's transport is NULL the
 * built-in epic-bus transports are used (I2C mem ops for the MCP23017,
 * raw SPI ops with the control-byte framing for the MCP23S17). The
 * callbacks mirror the epic-bus mem idiom: -1 on NACK/error, n on
 * success.
 */
typedef struct {
    int (*read_reg)(void *ctx, uint8_t reg, uint8_t *buf, int n);
    int (*write_reg)(void *ctx, uint8_t reg, const uint8_t *buf, int n);
    void *ctx;
} epic_mcp23x17_transport_t;

/**
 * Expander handle (the caller owns it). `dev` is the 7-bit I2C
 * address for BUS_I2C (0b0100A2A1A0) or the A2A1A0 pin value (0-7)
 * for BUS_SPI; with HAEN=0 (the default) the SPI control byte's
 * address bits are 0, so dev=0 works for any board wiring.
 */
typedef struct {
    epic_mcp23x17_bus_t bus;
    uint8_t dev;
    const epic_mcp23x17_transport_t *transport;  /**< NULL = built-in */
} epic_mcp23x17_handle_t;

/** Pin masks, matching the HAL GPIO_PIN_* values so the swap between
 *  the MCU's own pins and the expander is a prefix change. */
#define MCP23X17_PIN_0     ((uint16_t)1u << 0)
#define MCP23X17_PIN_1     ((uint16_t)1u << 1)
#define MCP23X17_PIN_2     ((uint16_t)1u << 2)
#define MCP23X17_PIN_3     ((uint16_t)1u << 3)
#define MCP23X17_PIN_4     ((uint16_t)1u << 4)
#define MCP23X17_PIN_5     ((uint16_t)1u << 5)
#define MCP23X17_PIN_6     ((uint16_t)1u << 6)
#define MCP23X17_PIN_7     ((uint16_t)1u << 7)
#define MCP23X17_PIN_All   ((uint16_t)0xFFu)

/** Pin state, mirroring the HAL's GPIO_PinState values. */
typedef enum {
    MCP23X17_PIN_RESET = 0,
    MCP23X17_PIN_SET   = 1,
} epic_mcp23x17_pin_state_t;

/** Direction modes for the GPIO-mimic Init, mapping to IODIR and
 *  GPPU (INPUT_PULLUP enables the 100k pull-ups on the pins). */
typedef enum {
    MCP23X17_MODE_OUTPUT = 0,
    MCP23X17_MODE_INPUT  = 1,
    MCP23X17_MODE_INPUT_PULLUP = 2,
} epic_mcp23x17_mode_t;

/**
 * @brief Configure @p pins on @p port as @p mode.
 *
 * The direction and pull-up registers are read-modify-written so pins
 * outside @p pins keep their configuration.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param pins bitmask of pins to configure
 * @param mode direction and pull-up mode to apply
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GPIO_Init(epic_mcp23x17_handle_t *h,
                            epic_mcp23x17_port_t port,
                            uint16_t pins, epic_mcp23x17_mode_t mode);

/**
 * @brief Write @p state to @p pins on @p port.
 *
 * Because the expander's GPIO register is a whole byte, this is a
 * read-modify-write of the output latch: two bus transactions with a
 * window between them. A concurrent writer to the same port in that
 * window can be lost (documented in README.md); use the whole-port
 * WritePort for atomic single-transaction writes.
 *
 * @param h the expander handle
 * @param port which 8-bit port to write
 * @param pins bitmask of pins to set or reset
 * @param state MCP23X17_PIN_SET or MCP23X17_PIN_RESET
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GPIO_WritePin(epic_mcp23x17_handle_t *h,
                                epic_mcp23x17_port_t port,
                                uint16_t pins,
                                epic_mcp23x17_pin_state_t state);

/**
 * @brief Toggle @p pins on @p port.
 *
 * Read-modify-write of the latch, with the same non-atomic window as
 * WritePin.
 *
 * @param h the expander handle
 * @param port which 8-bit port to toggle
 * @param pins bitmask of pins to invert
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GPIO_TogglePin(epic_mcp23x17_handle_t *h,
                                 epic_mcp23x17_port_t port,
                                 uint16_t pins);

/**
 * @brief Read the state of @p pin (a single pin) on @p port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pin bitmask of the single pin to test
 * @return MCP23X17_PIN_SET (1) or MCP23X17_PIN_RESET (0), or -1 when
 *         the device NACKs
 */
int EPIC_MCP23X17_GPIO_ReadPin(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port,
                               uint16_t pin);

/**
 * @brief Bind @p h to @p bus with device address @p dev.
 *
 * Uses the built-in transport.
 *
 * @param h the expander handle to bind
 * @param bus which serial bus the part hangs on
 * @param dev the 7-bit I2C address (BUS_I2C) or A2A1A0 value (BUS_SPI)
 * @return 0
 */
int EPIC_MCP23X17_Init(epic_mcp23x17_handle_t *h,
                       epic_mcp23x17_bus_t bus, uint8_t dev);

/**
 * @brief Bind @p h to a custom transport.
 *
 * h->bus is informational only.
 *
 * @param h the expander handle to bind
 * @param t the injectable register transport callbacks
 * @return 0
 */
int EPIC_MCP23X17_InitTransport(epic_mcp23x17_handle_t *h,
                                const epic_mcp23x17_transport_t *t);

/**
 * @brief Set the IODIR register: 1 = input, 0 = output per bit.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param dir direction bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetDirection(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port, uint8_t dir);
/**
 * @brief Read the IODIR register into @p dir.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param dir where the direction bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetDirection(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port, uint8_t *dir);
/**
 * @brief Set the IPOL register: 1 = inverted input polarity per bit.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param pol polarity bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInputPolarity(epic_mcp23x17_handle_t *h,
                                   epic_mcp23x17_port_t port, uint8_t pol);
/**
 * @brief Read the IPOL register into @p pol.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pol where the polarity bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetInputPolarity(epic_mcp23x17_handle_t *h,
                                   epic_mcp23x17_port_t port, uint8_t *pol);
/**
 * @brief Set the GPPU register: 1 = 100k pull-up on input pins.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param pu pull-up bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetPullUps(epic_mcp23x17_handle_t *h,
                             epic_mcp23x17_port_t port, uint8_t pu);
/**
 * @brief Read the GPPU register into @p pu.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pu where the pull-up bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetPullUps(epic_mcp23x17_handle_t *h,
                             epic_mcp23x17_port_t port, uint8_t *pu);

/**
 * @brief Write the output latch (the pins configured as outputs).
 *
 * @param h the expander handle
 * @param port which 8-bit port to write
 * @param val the output bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_WritePort(epic_mcp23x17_handle_t *h,
                            epic_mcp23x17_port_t port, uint8_t val);
/**
 * @brief Read the port pins into @p val.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param val where the pin bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadPort(epic_mcp23x17_handle_t *h,
                           epic_mcp23x17_port_t port, uint8_t *val);
/**
 * @brief Read the output latch (not the pins) into @p val.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param val where the latch bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadOutputLatch(epic_mcp23x17_handle_t *h,
                                  epic_mcp23x17_port_t port, uint8_t *val);

/**
 * @brief Set IODIR for both ports at once (16-bit, A in the low byte).
 *
 * @param h the expander handle
 * @param dir direction bitmask for both ports
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetDirectionAll(epic_mcp23x17_handle_t *h, uint16_t dir);
/**
 * @brief Read IODIR for both ports at once into @p dir.
 *
 * @param h the expander handle
 * @param dir where the 16-bit direction bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetDirectionAll(epic_mcp23x17_handle_t *h, uint16_t *dir);
/**
 * @brief Write the output latch of both ports at once.
 *
 * @param h the expander handle
 * @param val the 16-bit output bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_WriteAll(epic_mcp23x17_handle_t *h, uint16_t val);
/**
 * @brief Read both port pins at once into @p val.
 *
 * @param h the expander handle
 * @param val where the 16-bit pin bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadAll(epic_mcp23x17_handle_t *h, uint16_t *val);
/**
 * @brief Set GPPU for both ports at once (16-bit, A in the low byte).
 *
 * @param h the expander handle
 * @param pu pull-up bitmask for both ports
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetPullUpsAll(epic_mcp23x17_handle_t *h, uint16_t pu);

/**
 * @brief Write the IOCON register.
 *
 * @param h the expander handle
 * @param iocon the configuration byte to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetConfig(epic_mcp23x17_handle_t *h, uint8_t iocon);
/**
 * @brief Read the IOCON register into @p iocon.
 *
 * @param h the expander handle
 * @param iocon where the configuration byte is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetConfig(epic_mcp23x17_handle_t *h, uint8_t *iocon);

/**
 * @brief Set GPINTEN: 1 = interrupt-on-change enabled per pin.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param mask enable bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptEnable(epic_mcp23x17_handle_t *h,
                                     epic_mcp23x17_port_t port, uint8_t mask);
/**
 * @brief Set DEFVAL: the compare value for INTCON=1 pins.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param val the default-compare bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptDefault(epic_mcp23x17_handle_t *h,
                                      epic_mcp23x17_port_t port, uint8_t val);
/**
 * @brief Set INTCON: 1 = compare to DEFVAL, 0 = interrupt on change.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param mask control bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptControl(epic_mcp23x17_handle_t *h,
                                      epic_mcp23x17_port_t port, uint8_t mask);
/**
 * @brief Read INTF (the pending interrupt flags) into @p flags.
 *
 * Clears nothing.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param flags where the pending-flag bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadInterruptFlag(epic_mcp23x17_handle_t *h,
                                    epic_mcp23x17_port_t port, uint8_t *flags);
/**
 * @brief Read INTCAP (the port value captured at the interrupt).
 *
 * The flag and capture are cleared by a read of GPIO or INTCAP (the
 * part's own semantics; the module does not add hidden reads).
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param capture where the captured port value is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadInterruptCapture(epic_mcp23x17_handle_t *h,
                                       epic_mcp23x17_port_t port,
                                       uint8_t *capture);

#ifdef __cplusplus
}
#endif

#endif /* EPIC_MCP23X17_H */
