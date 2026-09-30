/* Family-agnostic I2C/SPI "MEM" register-access idiom (the
 * HAL_I2C_Mem_Read/Mem_Write layer Cube sensor code uses) on the
 * MSSP/SSP HAL, via a small injectable "bus ops" interface that also
 * lets the host test wire in a mock device. See docs/API.md
 * for the transaction shapes. */

#ifndef EPIC_BUS_H
#define EPIC_BUS_H

#include <stdint.h>

/* I2C */

/** I2C bus operations (the MEM logic calls these). The default implementation
 *  wraps the HAL SSP driver; inject your own (e.g. a mock) for testing. */
typedef struct {
    void    (*start)(void);
    void    (*repeated_start)(void);
    void    (*stop)(void);
    int     (*write_byte)(uint8_t b);   /**< returns 1 if slave ACKed, 0 if NACK */
    uint8_t (*read_byte)(int ack);      /**< reads one byte; ack=1 sends ACK, 0 sends NACK */
} epic_bus_i2c_ops_t;

/**
 * @brief  Configure the MSSP as an I2C master at @p fscl_hz (from
 *         @p fosc_hz) and use the default (HAL) I2C ops. Call once
 *         before the mem_read/write.
 * @param fosc_hz system oscillator frequency in Hz.
 * @param fscl_hz desired I2C SCL frequency in Hz.
 */
void epic_bus_i2c_init(uint32_t fosc_hz, uint32_t fscl_hz);

/**
 * @brief  Use a custom I2C ops table (NULL restores the default HAL ops).
 * @param ops the ops table to install, or NULL for the HAL default.
 */
void epic_bus_set_i2c_ops(const epic_bus_i2c_ops_t *ops);

/**
 * @brief  The active I2C ops table (the default HAL ops unless overridden).
 * @return the currently installed I2C ops table.
 */
const epic_bus_i2c_ops_t *epic_bus_get_i2c_ops(void);

/**
 * @brief  Write @p n bytes to register @p reg on I2C device @p dev
 *         (7-bit address). Transaction: START, (dev<<1)|W, reg,
 *         data..., STOP. Returns n on success (all ACKed), or -1 if the
 *         device NACKed the address or register.
 * @param dev  the 7-bit I2C device address.
 * @param reg  the register address to write.
 * @param data the bytes to write.
 * @param n    number of bytes in `data`.
 * @return n on success (all ACKed), or -1 if the device NACKed the
 *         address or register.
 */
int epic_bus_i2c_mem_write(uint8_t dev, uint8_t reg, const uint8_t *data, int n);

/**
 * @brief  Read @p n bytes from register @p reg on I2C device @p dev
 *         (7-bit address). Transaction: START, (dev<<1)|W, reg,
 *         REPEATED-START, (dev<<1)|R, read n-1 with ACK, read last with
 *         NACK, STOP. Returns n on success, -1 on address/register NACK.
 * @param dev the 7-bit I2C device address.
 * @param reg the register address to read.
 * @param buf the buffer that receives the read bytes.
 * @param n   number of bytes to read.
 * @return n on success, -1 on address/register NACK.
 */
int epic_bus_i2c_mem_read(uint8_t dev, uint8_t reg, uint8_t *buf, int n);

/* SPI */

/** SPI bus operations. The default wraps the HAL SSP driver + a GPIO CS. */
typedef struct {
    void    (*select)(void);
    void    (*deselect)(void);
    uint8_t (*exchange)(uint8_t b);     /**< write MOSI byte, return MISO byte shifted in */
} epic_bus_spi_ops_t;

/**
 * @brief  Configure the MSSP as an SPI master and use @p cs_port/@p cs_pin
 *         (GPIO) as the chip-select (asserted low). Uses the default
 *         (HAL) SPI ops.
 * @param fosc_hz   system oscillator frequency in Hz.
 * @param f_sclk_hz desired SPI SCLK frequency in Hz (0 selects the
 *                  coarsest available divider).
 * @param cs_port   GPIO port index of the chip-select pin.
 * @param cs_pin    bit index 0..7 of the chip-select pin.
 */
void epic_bus_spi_init(uint32_t fosc_hz, uint32_t f_sclk_hz, uint8_t cs_port, uint8_t cs_pin);

/**
 * @brief  Use a custom SPI ops table (NULL restores the default HAL ops).
 * @param ops the ops table to install, or NULL for the HAL default.
 */
void epic_bus_set_spi_ops(const epic_bus_spi_ops_t *ops);

/**
 * @brief  The active SPI ops table (the default HAL ops unless overridden).
 * @return the currently installed SPI ops table.
 */
const epic_bus_spi_ops_t *epic_bus_get_spi_ops(void);

/**
 * @brief  Write @p n bytes to register @p reg over SPI: CS low,
 *         exchange(reg), exchange(data[0..n-1]), CS high.
 * @param reg  the register address to write.
 * @param data the bytes to write.
 * @param n    number of bytes in `data`.
 * @return n.
 */
int epic_bus_spi_mem_write(uint8_t reg, const uint8_t *data, int n);

/**
 * @brief  Read @p n bytes from register @p reg over SPI: CS low,
 *         exchange(reg), exchange(0) x n (capturing MISO), CS high.
 * @param reg the register address to read.
 * @param buf the buffer that receives the read bytes.
 * @param n   number of bytes to read.
 * @return n.
 */
int epic_bus_spi_mem_read(uint8_t reg, uint8_t *buf, int n);

#endif /* EPIC_BUS_H */
