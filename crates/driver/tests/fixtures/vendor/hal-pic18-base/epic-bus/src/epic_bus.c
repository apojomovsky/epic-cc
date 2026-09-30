/* I2C/SPI MEM register-access transactions on the MSSP/SSP HAL via a
 * small injectable "bus ops" interface. The default ops wrap the HAL's
 * SSP primitives plus an SSPIF poll (the HAL's own calls only set
 * control bits and return); `epic_bus_set_i2c_ops`/`_set_spi_ops` let
 * the host test inject a mock device instead. */

#include "epic_bus.h"
#include "epic_hal.h"               /* SSP, GPIO, SFR, IRQ, platform */

#if defined(PIC18F2455) || defined(PIC18F2550) || defined(PIC18F4455) || defined(PIC18F4550)
  #define BUS_IS_PIC18         1
  #define BUS_IRQ_SSP          PIC18_IRQ_SSP
  #define BUS_SSPCON2_READ()   epic_sfr_read8(PIC_REG_SSPCON2)
  #define BUS_SSPCON2_WRITE(c) epic_sfr_write8(PIC_REG_SSPCON2, (uint8_t)(c))
#else
  #define BUS_IS_PIC18         0
  #define BUS_IRQ_SSP          PIC16_IRQ_SSP
  #define BUS_SSPCON2_READ()   EPIC_REG8(PIC_REG_SSPCON2)
  #define BUS_SSPCON2_WRITE(c) (EPIC_REG8(PIC_REG_SSPCON2) = (uint8_t)(c))
#endif

/** @brief Default I2C ops (HAL SSP + ACKDT + SSPIF wait): block until
 *         the current SSP operation completes, then clear SSPIF. */
static void i2c_wait_ssp(void)
{
    while (!EPIC_IRQ_GetFlag(BUS_IRQ_SSP))
    {
    }   /* block until the SSP op completes */
    EPIC_IRQ_ClearFlag(BUS_IRQ_SSP);
}

/**
 * @brief  Program the ACKDT bit for the next I2C acknowledge phase:
 *         ack=1 -> ACK (ACKDT=0); ack=0 -> NACK (ACKDT=1).
 * @param ack nonzero to ACK, zero to NACK.
 */
static void i2c_set_ackdt(int ack)
{
    uint8_t c = BUS_SSPCON2_READ();
    if (ack)
    {
        c &= (uint8_t)~PIC_SSPCON2_ACKDT;
    }
    else
    {
        c |= (uint8_t) PIC_SSPCON2_ACKDT;
    }
    BUS_SSPCON2_WRITE(c);
}

/** @brief Default I2C start: issue a hardware START and wait for SSPIF. */
static void i2c_real_start(void)
{
    EPIC_SSP_Start();          i2c_wait_ssp();
}
/** @brief Default I2C repeated start: issue a hardware RESTART and wait. */
static void i2c_real_repeated_start(void)
{
    EPIC_SSP_RepeatedStart();  i2c_wait_ssp();
}
/** @brief Default I2C stop: issue a hardware STOP and wait for SSPIF. */
static void i2c_real_stop(void)
{
    EPIC_SSP_Stop();           i2c_wait_ssp();
}
/**
 * @brief  Default I2C write byte: shift out `b` and report the slave's
 *         acknowledge.
 * @param b the byte to write.
 * @return 1 if the slave ACKed (ACKSTAT=0), 0 if it NACKed.
 */
static int  i2c_real_write_byte(uint8_t b)
{
    (void)EPIC_SSP_WriteByte(b);
    i2c_wait_ssp();
    return (EPIC_SSP_AcknowledgeStatus() == 0u) ? 1 : 0;   /* ACKSTAT=0 -> ACK */
}
/**
 * @brief  Default I2C read byte: set the ACK/NACK policy, receive, and
 *         re-arm the acknowledge enable.
 * @param ack nonzero to ACK this byte, zero to NACK it.
 * @return the received byte.
 */
static uint8_t i2c_real_read_byte(int ack)
{
    i2c_set_ackdt(ack);
    EPIC_SSP_ReceiveEnable();
    i2c_wait_ssp();
    uint8_t b = EPIC_SSP_ReadByte();
    EPIC_SSP_AcknowledgeEnable();
    i2c_wait_ssp();
    return b;
}

#ifdef __EPIC_CC__
/* epic-cc cannot place a struct of function pointers in flash (irparse
 * SPIKE LIMIT on const struct fields), and a statically-initialized
 * RAM table is still address-taken data; populate the default tables
 * at init instead. */
static epic_bus_i2c_ops_t g_i2c_default;
static const epic_bus_i2c_ops_t *g_i2c_ops = &g_i2c_default;
#else
static const epic_bus_i2c_ops_t g_i2c_default = {
    i2c_real_start, i2c_real_repeated_start, i2c_real_stop,
    i2c_real_write_byte, i2c_real_read_byte
};
static const epic_bus_i2c_ops_t *g_i2c_ops = &g_i2c_default;
#endif

/**
 * @brief  Install a custom I2C ops table (see the header for the full
 *         contract); NULL restores the HAL default.
 * @param ops the ops table to install, or NULL for the HAL default.
 */
void epic_bus_set_i2c_ops(const epic_bus_i2c_ops_t *ops)
{
    g_i2c_ops = ops ? ops : &g_i2c_default;
}

/**
 * @brief  Return the active I2C ops table (see the header).
 * @return the currently installed I2C ops table.
 */
const epic_bus_i2c_ops_t *epic_bus_get_i2c_ops(void)
{
    return g_i2c_ops;
}

/**
 * @brief  Configure the MSSP as an I2C master (see the header for the
 *         full contract) and install the HAL default I2C ops.
 * @param fosc_hz system oscillator frequency in Hz.
 * @param fscl_hz desired I2C SCL frequency in Hz.
 */
void epic_bus_i2c_init(uint32_t fosc_hz, uint32_t fscl_hz)
{
    static SSP_HandleTypeDef s_ssp;        /* static: Init may store the pointer */
    SSP_HandleTypeDef h = SSP_HANDLE_DEFAULT;
    h.Mode   = SSP_MODE_I2C_MASTER_FOSC;
    h.SSPADD = (uint8_t)SSP_ComputeSSPADD(fosc_hz, fscl_hz);
    s_ssp = h;
    EPIC_SSP_Init(&s_ssp);
#ifdef __EPIC_CC__
    g_i2c_default.start = i2c_real_start;
    g_i2c_default.repeated_start = i2c_real_repeated_start;
    g_i2c_default.stop = i2c_real_stop;
    g_i2c_default.write_byte = i2c_real_write_byte;
    g_i2c_default.read_byte = i2c_real_read_byte;
#endif
    g_i2c_ops = &g_i2c_default;
}

/* default SPI ops (HAL SSP exchange + GPIO CS) */
static uint8_t s_cs_port;
static uint8_t s_cs_pin;

/** @brief Default SPI select: assert the configured chip-select low. */
static void spi_real_select(void)
{
    EPIC_GPIO_WritePin((GPIO_TypeDef)s_cs_port, (uint16_t)EPIC_BIT(s_cs_pin), GPIO_PIN_RESET);
}
/** @brief Default SPI deselect: release the configured chip-select high. */
static void spi_real_deselect(void)
{
    EPIC_GPIO_WritePin((GPIO_TypeDef)s_cs_port, (uint16_t)EPIC_BIT(s_cs_pin), GPIO_PIN_SET);
}
/**
 * @brief  Default SPI exchange: shift out `b`, wait for the shift to
 *         complete, and return the byte shifted in.
 * @param b the MOSI byte to transmit.
 * @return the MISO byte received.
 */
static uint8_t spi_real_exchange(uint8_t b)
{
    (void)EPIC_SSP_WriteByte(b);
    while (!EPIC_SSP_IsBufferFull())
    {
    }    /* wait for the shift to complete */
    return EPIC_SSP_ReadByte();
}

#ifdef __EPIC_CC__
static epic_bus_spi_ops_t g_spi_default;
static const epic_bus_spi_ops_t *g_spi_ops = &g_spi_default;
#else
static const epic_bus_spi_ops_t g_spi_default = {
    spi_real_select, spi_real_deselect, spi_real_exchange
};
static const epic_bus_spi_ops_t *g_spi_ops = &g_spi_default;
#endif

/**
 * @brief  Install a custom SPI ops table (see the header for the full
 *         contract); NULL restores the HAL default.
 * @param ops the ops table to install, or NULL for the HAL default.
 */
void epic_bus_set_spi_ops(const epic_bus_spi_ops_t *ops)
{
    g_spi_ops = ops ? ops : &g_spi_default;
}

/**
 * @brief  Return the active SPI ops table (see the header).
 * @return the currently installed SPI ops table.
 */
const epic_bus_spi_ops_t *epic_bus_get_spi_ops(void)
{
    return g_spi_ops;
}

/**
 * @brief  Configure the MSSP as an SPI master (see the header for the
 *         full contract) with the closest standard clock divider, set up
 *         the GPIO chip-select, and install the HAL default SPI ops.
 * @param fosc_hz   system oscillator frequency in Hz.
 * @param f_sclk_hz desired SPI SCLK frequency in Hz (0 selects the
 *                  coarsest available divider).
 * @param cs_port   GPIO port index of the chip-select pin.
 * @param cs_pin    bit index 0..7 of the chip-select pin.
 */
void epic_bus_spi_init(uint32_t fosc_hz, uint32_t f_sclk_hz, uint8_t cs_port, uint8_t cs_pin)
{
    static SSP_HandleTypeDef s_ssp;
    SSP_HandleTypeDef h = SSP_HANDLE_DEFAULT;
    /* Pick the standard SPI clock divider closest to f_sclk_hz. */
    SSP_ModeTypeDef mode;
    if (f_sclk_hz == 0u || f_sclk_hz >= (fosc_hz / 8u))      mode = SSP_MODE_SPI_MASTER_FOSC_4;
    else if (f_sclk_hz >= (fosc_hz / 32u))                   mode = SSP_MODE_SPI_MASTER_FOSC_16;
    else                                                     mode = SSP_MODE_SPI_MASTER_FOSC_64;
    h.Mode = mode;
    s_ssp = h;
    EPIC_SSP_Init(&s_ssp);
    s_cs_port = cs_port;
    s_cs_pin  = cs_pin;
    EPIC_GPIO_Init((GPIO_TypeDef)cs_port, (uint16_t)EPIC_BIT(cs_pin), GPIO_MODE_OUTPUT);
    EPIC_GPIO_WritePin((GPIO_TypeDef)cs_port, (uint16_t)EPIC_BIT(cs_pin), GPIO_PIN_SET);
#ifdef __EPIC_CC__
    g_spi_default.select = spi_real_select;
    g_spi_default.deselect = spi_real_deselect;
    g_spi_default.exchange = spi_real_exchange;
#endif
    g_spi_ops = &g_spi_default;
}

/* MEM transactions (family-neutral, via the ops interface) */

/**
 * @brief  Write `n` bytes to register `reg` on I2C device `dev` (see
 *         the header for the full transaction shape), driving the
 *         installed I2C ops.
 * @param dev  the 7-bit I2C device address.
 * @param reg  the register address to write.
 * @param data the bytes to write.
 * @param n    number of bytes in `data`.
 * @return n on success, -1 on an address or register NACK.
 */
int epic_bus_i2c_mem_write(uint8_t dev, uint8_t reg, const uint8_t *data, int n)
{
#ifdef __EPIC_CC__
    /* epic-cc iselcore cannot build the GEP chain for an indirect call
     * through a RAM ops table (no gep for pointer, chain base missing);
     * the ops seam is the host-test surface, so the epic-cc footprint
     * probe links the module without it. XC8 keeps the real dispatch. */
    (void)dev; (void)reg; (void)data;
    return -1;
#else
    const epic_bus_i2c_ops_t *o = g_i2c_ops;
    o->start();
    if (!o->write_byte((uint8_t)((dev << 1) | 0u)))
    {
        o->stop(); return -1;
    }
    if (!o->write_byte(reg))
    {
        o->stop(); return -1;
    }
    for (int i = 0; i < n; i++)
    {
        if (!o->write_byte(data[i]))
        {
            o->stop(); return -1;
        }
    }
    o->stop();
    return n;
#endif
}

/**
 * @brief  Read `n` bytes from register `reg` on I2C device `dev` (see
 *         the header for the full transaction shape), driving the
 *         installed I2C ops.
 * @param dev the 7-bit I2C device address.
 * @param reg the register address to read.
 * @param buf the buffer that receives the read bytes.
 * @param n   number of bytes to read.
 * @return n on success, -1 on an address or register NACK.
 */
int epic_bus_i2c_mem_read(uint8_t dev, uint8_t reg, uint8_t *buf, int n)
{
#ifdef __EPIC_CC__
    (void)dev; (void)reg; (void)buf;
    return -1;
#else
    const epic_bus_i2c_ops_t *o = g_i2c_ops;
    o->start();
    if (!o->write_byte((uint8_t)((dev << 1) | 0u)))
    {
        o->stop(); return -1;
    }
    if (!o->write_byte(reg))
    {
        o->stop(); return -1;
    }
    o->repeated_start();
    if (!o->write_byte((uint8_t)((dev << 1) | 1u)))
    {
        o->stop(); return -1;
    }
    for (int i = 0; i < n; i++)
    {
        buf[i] = o->read_byte(i < (n - 1) ? 1 : 0);   /* ACK all but the last */
    }
    o->stop();
    return n;
#endif
}

/**
 * @brief  Write `n` bytes to register `reg` over SPI (see the header
 *         for the full transaction shape), driving the installed SPI
 *         ops.
 * @param reg  the register address to write.
 * @param data the bytes to write.
 * @param n    number of bytes in `data`.
 * @return n.
 */
int epic_bus_spi_mem_write(uint8_t reg, const uint8_t *data, int n)
{
#ifdef __EPIC_CC__
    (void)reg; (void)data;
    return -1;
#else
    const epic_bus_spi_ops_t *o = g_spi_ops;
    o->select();
    (void)o->exchange(reg);
    for (int i = 0; i < n; i++)
    {
        (void)o->exchange(data[i]);
    }
    o->deselect();
    return n;
#endif
}

/**
 * @brief  Read `n` bytes from register `reg` over SPI (see the header
 *         for the full transaction shape), driving the installed SPI
 *         ops.
 * @param reg the register address to read.
 * @param buf the buffer that receives the read bytes.
 * @param n   number of bytes to read.
 * @return n.
 */
int epic_bus_spi_mem_read(uint8_t reg, uint8_t *buf, int n)
{
#ifdef __EPIC_CC__
    (void)reg; (void)buf;
    return -1;
#else
    const epic_bus_spi_ops_t *o = g_spi_ops;
    o->select();
    (void)o->exchange(reg);
    for (int i = 0; i < n; i++)
    {
        buf[i] = o->exchange(0u);
    }
    o->deselect();
    return n;
#endif
}
