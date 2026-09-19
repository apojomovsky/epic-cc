/**
 * HAL-dependent transport types and init functions for epic_lcd, kept out
 * of epic_lcd.h so the host test build (no HAL) only needs the ops/config
 * types. Include when using the GPIO or SPI transports on target.
 */

#ifndef EPIC_LCD_TRANSPORT_H
#define EPIC_LCD_TRANSPORT_H

#include "epic_lcd.h"
#include "epic_hal.h"

typedef struct {
    GPIO_TypeDef rs_port;  uint16_t rs_pin;
    GPIO_TypeDef e_port;   uint16_t e_pin;
    GPIO_TypeDef db4_port; uint16_t db4_pin;
    GPIO_TypeDef db5_port; uint16_t db5_pin;
    GPIO_TypeDef db6_port; uint16_t db6_pin;
    GPIO_TypeDef db7_port; uint16_t db7_pin;
} epic_lcd_gpio4_pins_t;

/**
 * @brief Initialize the 4-bit parallel GPIO transport.
 *
 * Configures the RS/E/DB4-DB7 pins as outputs, asserts E low, and binds
 * the transport's send/delay ops into ops.
 *
 * @param ops   transport ops struct to fill in
 * @param ctx   receives the transport context pointer to pass to ops calls
 * @param pins  GPIO pin assignments for RS, E, and DB4-DB7
 */
void epic_lcd_gpio4_init(epic_lcd_ops_t *ops, void **ctx,
                         const epic_lcd_gpio4_pins_t *pins);

typedef struct {
    GPIO_TypeDef rs_port;  uint16_t rs_pin;
    GPIO_TypeDef e_port;   uint16_t e_pin;
    GPIO_TypeDef db0_port; uint16_t db0_pin;
    GPIO_TypeDef db1_port; uint16_t db1_pin;
    GPIO_TypeDef db2_port; uint16_t db2_pin;
    GPIO_TypeDef db3_port; uint16_t db3_pin;
    GPIO_TypeDef db4_port; uint16_t db4_pin;
    GPIO_TypeDef db5_port; uint16_t db5_pin;
    GPIO_TypeDef db6_port; uint16_t db6_pin;
    GPIO_TypeDef db7_port; uint16_t db7_pin;
} epic_lcd_gpio8_pins_t;

/**
 * @brief Initialize the 8-bit parallel GPIO transport.
 *
 * Configures the RS/E/DB0-DB7 pins as outputs, asserts E low, and binds
 * the transport's send/delay ops into ops.
 *
 * @param ops   transport ops struct to fill in
 * @param ctx   receives the transport context pointer to pass to ops calls
 * @param pins  GPIO pin assignments for RS, E, and DB0-DB7
 */
void epic_lcd_gpio8_init(epic_lcd_ops_t *ops, void **ctx,
                         const epic_lcd_gpio8_pins_t *pins);

typedef struct {
    uint8_t rs_bit;
    uint8_t e_bit;
    uint8_t db4_bit;
    uint8_t db5_bit;
    uint8_t db6_bit;
    uint8_t db7_bit;
    uint8_t rw_bit;    /* 0xFF = not connected (tied low) */
} epic_lcd_spi_layout_t;

extern const epic_lcd_spi_layout_t EPIC_LCD_SPI_LAYOUT_COMMON;

typedef struct {
    GPIO_TypeDef cs_port;  uint16_t cs_pin;
    uint32_t fosc_hz;
    uint32_t spi_hz;
} epic_lcd_spi_config_t;

/**
 * @brief Initialize the SPI transport via a 74HC595 shift register.
 *
 * Configures the SSP as SPI master, drives CS idle, and binds the
 * transport's send/delay ops into ops.
 *
 * @param ops    transport ops struct to fill in
 * @param ctx    receives the transport context pointer to pass to ops calls
 * @param config CS pin and clock configuration
 * @param layout mapping of 595 Q outputs to LCD signals
 */
void epic_lcd_spi_init(epic_lcd_ops_t *ops, void **ctx,
                       const epic_lcd_spi_config_t *config,
                       const epic_lcd_spi_layout_t *layout);

#endif /* EPIC_LCD_TRANSPORT_H */
