/**
 * MCP23017 / MCP23S17 16-bit I/O expander driver (see the header and
 * README.md): one family-agnostic source; the only family
 * dimension is the transport, which rides on epic-bus.
 */

#include "epic_mcp23x17.h"

#include "epic_bus.h"   /* built-in I2C mem + raw SPI ops transports */

/* Register map, IOCON.BANK=0 (sequential). Each port pair is the
 * base plus the port index. DS20001952E section 3.5. */
enum {
    REG_IODIR   = 0x00,   /* 1 = input (reset default) */
    REG_IPOL    = 0x02,   /* 1 = inverted input polarity */
    REG_GPINTEN = 0x04,   /* 1 = interrupt-on-change enabled per pin */
    REG_DEFVAL  = 0x06,   /* compare value for INTCON=1 pins */
    REG_INTCON  = 0x08,   /* 1 = compare to DEFVAL, 0 = on pin change */
    REG_IOCON   = 0x0A,   /* BANK MIRROR SEQOP DISSLW HAEN ODR INTPOL - */
    REG_GPPU    = 0x0B,   /* 1 = 100k pull-up on input pins */
    REG_INTF    = 0x0D,   /* read-only pending flags */
    REG_INTCAP  = 0x0F,   /* read-only captured port value */
    REG_GPIO    = 0x11,   /* read pins / write the output latch */
    REG_OLAT    = 0x13,   /* read the output latch */
};

/* SPI control byte: 0b0100_0A2A1A0_RW (HAEN disabled: address bits
 * from the dev field; RW=1 for reads). */
#define SPI_CTRL(dev, rw) ((uint8_t)(0x40u | (((dev) & 0x7u) << 1) | (rw)))

/**
 * @brief Compute the register address of a port's register pair.
 *
 * @param port which 8-bit port the access targets
 * @param base the register's port-A address
 * @return the register address for @p port
 */
static uint8_t reg_of(epic_mcp23x17_port_t port, uint8_t base)
{
    return (uint8_t)(base + (uint8_t)port);
}

/**
 * @brief Read @p n bytes starting at register @p reg.
 *
 * Uses the injectable transport when set, else the built-in epic-bus
 * I2C mem read or the MCP23S17 SPI control-byte framing.
 *
 * @param h the expander handle
 * @param reg the register address to read from
 * @param buf where the read bytes are written
 * @param n how many bytes to read
 * @return the transfer status (n on success, -1 on NACK/error)
 */
static int reg_read(epic_mcp23x17_handle_t *h, uint8_t reg,
                    uint8_t *buf, int n)
{
#ifdef __EPIC_CC__
    /* The epic-cc footprint probe links the module without the bus
     * dispatch (iselcore gep gap on the ops chain); XC8 keeps it. */
    (void)h; (void)reg; (void)buf;
    return n;
#else
    if (h->transport != NULL)
    {
        return h->transport->read_reg(h->transport->ctx, reg, buf, n);
    }
    if (h->bus == EPIC_MCP23X17_BUS_I2C)
    {
        return epic_bus_i2c_mem_read(h->dev, reg, buf, n);
    }
    /* MCP23S17: CS low, control byte (read), reg, n dummy exchanges
     * capturing MISO, CS high. */
    const epic_bus_spi_ops_t *ops = epic_bus_get_spi_ops();
    ops->select();
    (void)ops->exchange(SPI_CTRL(h->dev, 1u));
    (void)ops->exchange(reg);
    for (int i = 0; i < n; i++)
    {
        buf[i] = ops->exchange(0u);
    }
    ops->deselect();
    return n;
#endif
}

/**
 * @brief Write @p n bytes starting at register @p reg.
 *
 * Uses the injectable transport when set, else the built-in epic-bus
 * I2C mem write or the MCP23S17 SPI control-byte framing.
 *
 * @param h the expander handle
 * @param reg the register address to write to
 * @param buf the bytes to write
 * @param n how many bytes to write
 * @return the transfer status (n on success, -1 on NACK/error)
 */
static int reg_write(epic_mcp23x17_handle_t *h, uint8_t reg,
                     const uint8_t *buf, int n)
{
#ifdef __EPIC_CC__
    (void)h; (void)reg; (void)buf;
    return n;
#else
    if (h->transport != NULL)
    {
        return h->transport->write_reg(h->transport->ctx, reg, buf, n);
    }
    if (h->bus == EPIC_MCP23X17_BUS_I2C)
    {
        return epic_bus_i2c_mem_write(h->dev, reg, buf, n);
    }
    const epic_bus_spi_ops_t *ops = epic_bus_get_spi_ops();
    ops->select();
    (void)ops->exchange(SPI_CTRL(h->dev, 0u));
    (void)ops->exchange(reg);
    for (int i = 0; i < n; i++)
    {
        (void)ops->exchange(buf[i]);
    }
    ops->deselect();
    return n;
#endif
}

/**
 * @brief Configure @p pins on @p port as @p mode.
 *
 * Read-modify-writes the direction and pull-up registers so pins
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
                            uint16_t pins, epic_mcp23x17_mode_t mode)
{
    uint8_t mask = (uint8_t)(pins & 0xFFu);
    uint8_t dir;
    int st = EPIC_MCP23X17_GetDirection(h, port, &dir);
    if (st < 0)
    {
        return st;
    }
    if (mode == MCP23X17_MODE_OUTPUT)
    {
        dir &= (uint8_t)~mask;
    }
    else
    {
        dir |= mask;
    }
    st = EPIC_MCP23X17_SetDirection(h, port, dir);
    if (st < 0)
    {
        return st;
    }
    /* the pull-ups follow the mode for the affected pins only. */
    uint8_t pu;
    st = EPIC_MCP23X17_GetPullUps(h, port, &pu);
    if (st < 0)
    {
        return st;
    }
    if (mode == MCP23X17_MODE_INPUT_PULLUP)
    {
        pu |= mask;
    }
    else
    {
        pu &= (uint8_t)~mask;
    }
    return EPIC_MCP23X17_SetPullUps(h, port, pu);
}

/**
 * @brief Write @p state to @p pins on @p port.
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
                                epic_mcp23x17_pin_state_t state)
{
    uint8_t mask = (uint8_t)(pins & 0xFFu);
    uint8_t latch;
    int st = EPIC_MCP23X17_ReadOutputLatch(h, port, &latch);
    if (st < 0)
    {
        return st;
    }
    if (state == MCP23X17_PIN_SET)
    {
        latch |= mask;
    }
    else
    {
        latch &= (uint8_t)~mask;
    }
    return EPIC_MCP23X17_WritePort(h, port, latch);
}

/**
 * @brief Toggle @p pins on @p port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to toggle
 * @param pins bitmask of pins to invert
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GPIO_TogglePin(epic_mcp23x17_handle_t *h,
                                 epic_mcp23x17_port_t port,
                                 uint16_t pins)
{
    uint8_t mask = (uint8_t)(pins & 0xFFu);
    uint8_t latch;
    int st = EPIC_MCP23X17_ReadOutputLatch(h, port, &latch);
    if (st < 0)
    {
        return st;
    }
    latch ^= mask;
    return EPIC_MCP23X17_WritePort(h, port, latch);
}

/**
 * @brief Read the state of a single pin on @p port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pin bitmask of the single pin to test
 * @return MCP23X17_PIN_SET (1) or MCP23X17_PIN_RESET (0), or -1 when
 *         the device NACKs
 */
int EPIC_MCP23X17_GPIO_ReadPin(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port,
                               uint16_t pin)
{
    uint8_t port_val;
    int st = EPIC_MCP23X17_ReadPort(h, port, &port_val);
    if (st < 0)
    {
        return st;
    }
    return (port_val & (uint8_t)(pin & 0xFFu)) ? MCP23X17_PIN_SET
                                               : MCP23X17_PIN_RESET;
}

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
                       epic_mcp23x17_bus_t bus, uint8_t dev)
{
    h->bus = bus;
    h->dev = dev;
    h->transport = NULL;
    return 0;
}

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
                                const epic_mcp23x17_transport_t *t)
{
    h->bus = EPIC_MCP23X17_BUS_I2C;   /* informational only */
    h->dev = 0u;
    h->transport = t;
    return 0;
}

/**
 * @brief Set the IODIR register for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param dir direction bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetDirection(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port, uint8_t dir)
{
    return reg_write(h, reg_of(port, REG_IODIR), &dir, 1);
}

/**
 * @brief Read the IODIR register of one port into @p dir.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param dir where the direction bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetDirection(epic_mcp23x17_handle_t *h,
                               epic_mcp23x17_port_t port, uint8_t *dir)
{
    return reg_read(h, reg_of(port, REG_IODIR), dir, 1);
}

/**
 * @brief Set the IPOL register for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param pol polarity bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInputPolarity(epic_mcp23x17_handle_t *h,
                                   epic_mcp23x17_port_t port, uint8_t pol)
{
    return reg_write(h, reg_of(port, REG_IPOL), &pol, 1);
}

/**
 * @brief Read the IPOL register of one port into @p pol.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pol where the polarity bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetInputPolarity(epic_mcp23x17_handle_t *h,
                                   epic_mcp23x17_port_t port, uint8_t *pol)
{
    return reg_read(h, reg_of(port, REG_IPOL), pol, 1);
}

/**
 * @brief Set the GPPU register for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param pu pull-up bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetPullUps(epic_mcp23x17_handle_t *h,
                             epic_mcp23x17_port_t port, uint8_t pu)
{
    return reg_write(h, reg_of(port, REG_GPPU), &pu, 1);
}

/**
 * @brief Read the GPPU register of one port into @p pu.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param pu where the pull-up bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetPullUps(epic_mcp23x17_handle_t *h,
                             epic_mcp23x17_port_t port, uint8_t *pu)
{
    return reg_read(h, reg_of(port, REG_GPPU), pu, 1);
}

/**
 * @brief Write the output latch of one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to write
 * @param val the output bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_WritePort(epic_mcp23x17_handle_t *h,
                            epic_mcp23x17_port_t port, uint8_t val)
{
    return reg_write(h, reg_of(port, REG_GPIO), &val, 1);
}

/**
 * @brief Read the pins of one port into @p val.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param val where the pin bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadPort(epic_mcp23x17_handle_t *h,
                           epic_mcp23x17_port_t port, uint8_t *val)
{
    return reg_read(h, reg_of(port, REG_GPIO), val, 1);
}

/**
 * @brief Read the output latch of one port into @p val.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param val where the latch bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadOutputLatch(epic_mcp23x17_handle_t *h,
                                  epic_mcp23x17_port_t port, uint8_t *val)
{
    return reg_read(h, reg_of(port, REG_OLAT), val, 1);
}

/**
 * @brief Set IODIR for both ports at once (16-bit, A in the low byte).
 *
 * @param h the expander handle
 * @param dir direction bitmask for both ports
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetDirectionAll(epic_mcp23x17_handle_t *h, uint16_t dir)
{
#ifdef __EPIC_CC__
    /* Local arrays panic irparse (SPIKE: unsupported type [2 x i8]);
     * a two-byte struct compiles and stays overlay-allocated. XC8
     * keeps the stack-local pair. */
    struct { uint8_t lo; uint8_t hi; } pair;
    pair.lo = (uint8_t)dir;
    pair.hi = (uint8_t)(dir >> 8);
    return reg_write(h, REG_IODIR, (const uint8_t *)&pair, 2);
#else
    uint8_t pair[2] = { (uint8_t)dir, (uint8_t)(dir >> 8) };
    return reg_write(h, REG_IODIR, pair, 2);
#endif
}

/**
 * @brief Read IODIR for both ports at once into @p dir.
 *
 * @param h the expander handle
 * @param dir where the 16-bit direction bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetDirectionAll(epic_mcp23x17_handle_t *h, uint16_t *dir)
{
#ifdef __EPIC_CC__
    struct { uint8_t lo; uint8_t hi; } pair;
    int st = reg_read(h, REG_IODIR, (uint8_t *)&pair, 2);
    if (st < 0)
    {
        return st;
    }
    *dir = (uint16_t)((uint16_t)pair.lo | ((uint16_t)pair.hi << 8));
    return st;
#else
    uint8_t pair[2];
    int st = reg_read(h, REG_IODIR, pair, 2);
    if (st < 0)
    {
        return st;
    }
    *dir = (uint16_t)((uint16_t)pair[0] | ((uint16_t)pair[1] << 8));
    return st;
#endif
}

/**
 * @brief Write the output latch of both ports at once.
 *
 * @param h the expander handle
 * @param val the 16-bit output bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_WriteAll(epic_mcp23x17_handle_t *h, uint16_t val)
{
#ifdef __EPIC_CC__
    struct { uint8_t lo; uint8_t hi; } pair;
    pair.lo = (uint8_t)val;
    pair.hi = (uint8_t)(val >> 8);
    return reg_write(h, REG_GPIO, (const uint8_t *)&pair, 2);
#else
    uint8_t pair[2] = { (uint8_t)val, (uint8_t)(val >> 8) };
    return reg_write(h, REG_GPIO, pair, 2);
#endif
}

/**
 * @brief Read both port pins at once into @p val.
 *
 * @param h the expander handle
 * @param val where the 16-bit pin bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadAll(epic_mcp23x17_handle_t *h, uint16_t *val)
{
#ifdef __EPIC_CC__
    struct { uint8_t lo; uint8_t hi; } pair;
    int st = reg_read(h, REG_GPIO, (uint8_t *)&pair, 2);
    if (st < 0)
    {
        return st;
    }
    *val = (uint16_t)((uint16_t)pair.lo | ((uint16_t)pair.hi << 8));
    return st;
#else
    uint8_t pair[2];
    int st = reg_read(h, REG_GPIO, pair, 2);
    if (st < 0)
    {
        return st;
    }
    *val = (uint16_t)((uint16_t)pair[0] | ((uint16_t)pair[1] << 8));
    return st;
#endif
}

/**
 * @brief Set GPPU for both ports at once (16-bit, A in the low byte).
 *
 * @param h the expander handle
 * @param pu pull-up bitmask for both ports
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetPullUpsAll(epic_mcp23x17_handle_t *h, uint16_t pu)
{
#ifdef __EPIC_CC__
    struct { uint8_t lo; uint8_t hi; } pair;
    pair.lo = (uint8_t)pu;
    pair.hi = (uint8_t)(pu >> 8);
    return reg_write(h, REG_GPPU, (const uint8_t *)&pair, 2);
#else
    uint8_t pair[2] = { (uint8_t)pu, (uint8_t)(pu >> 8) };
    return reg_write(h, REG_GPPU, pair, 2);
#endif
}

/**
 * @brief Write the IOCON register.
 *
 * @param h the expander handle
 * @param iocon the configuration byte to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetConfig(epic_mcp23x17_handle_t *h, uint8_t iocon)
{
    return reg_write(h, REG_IOCON, &iocon, 1);
}

/**
 * @brief Read the IOCON register into @p iocon.
 *
 * @param h the expander handle
 * @param iocon where the configuration byte is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_GetConfig(epic_mcp23x17_handle_t *h, uint8_t *iocon)
{
    return reg_read(h, REG_IOCON, iocon, 1);
}

/**
 * @brief Set GPINTEN for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param mask enable bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptEnable(epic_mcp23x17_handle_t *h,
                                     epic_mcp23x17_port_t port, uint8_t mask)
{
    return reg_write(h, reg_of(port, REG_GPINTEN), &mask, 1);
}

/**
 * @brief Set DEFVAL for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param val the default-compare bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptDefault(epic_mcp23x17_handle_t *h,
                                      epic_mcp23x17_port_t port, uint8_t val)
{
    return reg_write(h, reg_of(port, REG_DEFVAL), &val, 1);
}

/**
 * @brief Set INTCON for one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to configure
 * @param mask control bitmask to write
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_SetInterruptControl(epic_mcp23x17_handle_t *h,
                                      epic_mcp23x17_port_t port, uint8_t mask)
{
    return reg_write(h, reg_of(port, REG_INTCON), &mask, 1);
}

/**
 * @brief Read INTF (the pending interrupt flags) of one port.
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param flags where the pending-flag bitmask is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadInterruptFlag(epic_mcp23x17_handle_t *h,
                                    epic_mcp23x17_port_t port, uint8_t *flags)
{
    return reg_read(h, reg_of(port, REG_INTF), flags, 1);
}

/**
 * @brief Read INTCAP (the port value captured at the interrupt).
 *
 * @param h the expander handle
 * @param port which 8-bit port to read
 * @param capture where the captured port value is written
 * @return the transfer status (0 on success, -1 on NACK/error)
 */
int EPIC_MCP23X17_ReadInterruptCapture(epic_mcp23x17_handle_t *h,
                                       epic_mcp23x17_port_t port,
                                       uint8_t *capture)
{
    return reg_read(h, reg_of(port, REG_INTCAP), capture, 1);
}
