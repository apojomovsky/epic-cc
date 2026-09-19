/** HD44780 command core, transport-agnostic (ops injected at init). */

#include "epic_lcd.h"

#include <string.h>

/* HD44780 instruction bits */

#define CMD_CLEAR_DISPLAY      0x01u
#define CMD_RETURN_HOME        0x02u
#define CMD_ENTRY_MODE_SET     0x04u
#define CMD_DISPLAY_CTRL       0x08u
#define CMD_CURSOR_SHIFT       0x10u
#define CMD_FUNCTION_SET       0x20u
#define CMD_SET_CGRAM_ADDR     0x40u
#define CMD_SET_DDRAM_ADDR     0x80u

/* Entry mode bits */
#define ENTRY_INCREMENT        0x02u
#define ENTRY_SHIFT_DISPLAY    0x01u

/* Display control bits */
#define DISPLAY_ON             0x04u
#define DISPLAY_CURSOR         0x02u
#define DISPLAY_BLINK          0x01u

/* Cursor shift bits */
#define SHIFT_DISPLAY          0x08u
#define SHIFT_RIGHT            0x04u

/* Function set bits */
#define FS_8BIT                0x10u
#define FS_2LINE               0x08u
#define FS_5x10                0x04u

/* Execution times (ST7066 / HD44780, fosc=270kHz) */
#define DELAY_CLEAR_US         1600u   /* 1.53 ms, rounded up */
#define DELAY_HOME_US          1600u   /* 1.53 ms, rounded up */
#define DELAY_CMD_US           40u     /* 39 us, rounded up   */
#define DELAY_INIT_MS          50u     /* >40 ms after power-on */
#define DELAY_INIT4_US        4500u   /* >4.1 ms             */

/* helpers */

/**
 * @brief Send an instruction byte (RS=0) through the transport.
 *
 * @param lcd LCD instance whose transport to use
 * @param cmd HD44780 instruction byte
 */
static void send_cmd(epic_lcd_t *lcd, uint8_t cmd)
{
#ifdef __EPIC_CC__
    /* The ops-pointer chain hits an epic-cc iselcore gep gap (no gep
     * for pointer); the epic-cc footprint probe links the module
     * without the transport dispatch. XC8 keeps the real ops calls. */
    (void)lcd; (void)cmd;
#else
    lcd->ops->send(lcd->ops_ctx, 0, cmd);
#endif
}

/**
 * @brief Send a data byte (RS=1) through the transport.
 *
 * @param lcd  LCD instance whose transport to use
 * @param data DDRAM/CGRAM data byte
 */
static void send_data(epic_lcd_t *lcd, uint8_t data)
{
#ifdef __EPIC_CC__
    (void)lcd; (void)data;
#else
    lcd->ops->send(lcd->ops_ctx, 1, data);
#endif
}

/**
 * @brief Wait the short inter-command execution time (40 us).
 *
 * @param lcd LCD instance whose transport delay op to use
 */
static void cmd_short_wait(epic_lcd_t *lcd)
{
#ifdef __EPIC_CC__
    (void)lcd;
#else
    lcd->ops->delay_us(lcd->ops_ctx, DELAY_CMD_US);
#endif
}

/**
 * @brief Wait the long clear/home execution time (1.53 ms).
 *
 * @param lcd LCD instance whose transport delay op to use
 */
static void cmd_long_wait(epic_lcd_t *lcd)
{
#ifdef __EPIC_CC__
    (void)lcd;
#else
    lcd->ops->delay_us(lcd->ops_ctx, DELAY_CLEAR_US);
#endif
}

/* Row-address defaults for the standard HD44780 layout.
 * 16x2: row 0 = 0x00, row 1 = 0x40
 * 20x4: row 0 = 0x00, row 1 = 0x40, row 2 = 0x14, row 3 = 0x54 */
static const uint8_t default_row_addr[EPIC_LCD_MAX_ROWS] = {
    0x00u, 0x40u, 0x14u, 0x54u
};

/**
 * @brief Run the full HD44780 init sequence and apply the caller's
 *        display/cursor/blink and entry-mode defaults.
 *
 * Call once before any other function; ops/ops_ctx must outlive the lcd
 * instance.
 *
 * @param lcd      LCD instance to initialize
 * @param ops      transport ops (send/delay) the driver will use
 * @param ops_ctx  transport context passed to each ops call
 * @param config   display geometry and entry-mode configuration
 */
void epic_lcd_init(epic_lcd_t *lcd, const epic_lcd_ops_t *ops, void *ops_ctx,
                   const epic_lcd_config_t *config)
{
    lcd->ops       = ops;
    lcd->ops_ctx   = ops_ctx;
    lcd->cols      = config->cols;
    lcd->rows      = config->rows;
    lcd->display_ctrl = 0u;
    lcd->entry_mode   = ENTRY_INCREMENT;

#ifdef __EPIC_CC__
    /* The const-table memcpy hits an epic-cc isel gap (no address for
     * @default_row_addr); a loop compiles. XC8 keeps the memcpy. */
    for (uint8_t i = 0u; i < EPIC_LCD_MAX_ROWS; i++)
    {
        lcd->row_addr[i] = config->row_addr[i];
    }
    if (config->row_addr[0] == 0u)
    {
        for (uint8_t i = 0u; i < EPIC_LCD_MAX_ROWS; i++)
        {
            lcd->row_addr[i] = default_row_addr[i];
        }
    }
#else
    memcpy(lcd->row_addr, config->row_addr, EPIC_LCD_MAX_ROWS);
    if (config->row_addr[0] == 0u)
    {
        memcpy(lcd->row_addr, default_row_addr, EPIC_LCD_MAX_ROWS);
    }
#endif

    /* HD44780 8-bit init sequence per datasheet §13; the 4-bit transport
     * handles its own sub-sequence internally, so this always sends the
     * 8-bit-form Function Set regardless of the active transport. */

#ifdef __EPIC_CC__
    /* lcd is already used above (ops/cols/rows/row_addr); the delay
     * ops are stubbed, so nothing else to do here. */
#else
    lcd->ops->delay_ms(lcd->ops_ctx, DELAY_INIT_MS);
#endif

    send_cmd(lcd, CMD_FUNCTION_SET | FS_8BIT | FS_2LINE);
#ifdef __EPIC_CC__
    /* lcd is already used above; the delay ops are stubbed. */
#else
    lcd->ops->delay_us(lcd->ops_ctx, DELAY_INIT4_US);
#endif

    send_cmd(lcd, CMD_FUNCTION_SET | FS_8BIT | FS_2LINE);
    cmd_short_wait(lcd);

    send_cmd(lcd, CMD_FUNCTION_SET | FS_8BIT | FS_2LINE);
    cmd_short_wait(lcd);

    /* Display off (D=0); re-enabled after clear */
    send_cmd(lcd, CMD_DISPLAY_CTRL);
    cmd_short_wait(lcd);

    send_cmd(lcd, CMD_CLEAR_DISPLAY);
    cmd_long_wait(lcd);

    /* Entry mode: increment cursor, no display shift */
    send_cmd(lcd, CMD_ENTRY_MODE_SET | lcd->entry_mode);
    cmd_short_wait(lcd);

    /* Display on (cursor off, blink off) */
    lcd->display_ctrl = DISPLAY_ON;
    send_cmd(lcd, CMD_DISPLAY_CTRL | lcd->display_ctrl);
    cmd_short_wait(lcd);
}

/**
 * @brief Clear the entire display and return the cursor to row 0, col 0.
 *
 * Takes ~1.53 ms to execute (the driver waits internally).
 *
 * @param lcd LCD instance
 */
void epic_lcd_clear(epic_lcd_t *lcd)
{
    send_cmd(lcd, CMD_CLEAR_DISPLAY);
    cmd_long_wait(lcd);
}

/**
 * @brief Return the cursor to row 0, col 0.
 *
 * Display contents are not changed. Takes ~1.53 ms.
 *
 * @param lcd LCD instance
 */
void epic_lcd_home(epic_lcd_t *lcd)
{
    send_cmd(lcd, CMD_RETURN_HOME);
    cmd_long_wait(lcd);
}

/**
 * @brief Move the cursor to (col, row).
 *
 * Row 0 is the top row. Out-of-range coordinates clamp to the display
 * edge.
 *
 * @param lcd LCD instance
 * @param col column to move to
 * @param row row to move to
 */
void epic_lcd_set_cursor(epic_lcd_t *lcd, uint8_t col, uint8_t row)
{
    if (row >= lcd->rows)
    {
        row = (uint8_t)(lcd->rows - 1u);
    }
    if (col >= lcd->cols)
    {
        col = (uint8_t)(lcd->cols - 1u);
    }
    uint8_t addr = (uint8_t)(lcd->row_addr[row] + col);
    send_cmd(lcd, CMD_SET_DDRAM_ADDR | addr);
    cmd_short_wait(lcd);
}

/**
 * @brief Write one character at the current cursor position.
 *
 * @param lcd LCD instance
 * @param c   character to write
 */
void epic_lcd_write_char(epic_lcd_t *lcd, char c)
{
    send_data(lcd, (uint8_t)c);
    cmd_short_wait(lcd);
}

/**
 * @brief Write len bytes from str at the current cursor position.
 *
 * @param lcd LCD instance
 * @param str buffer to write
 * @param len number of bytes to write
 */
void epic_lcd_write(epic_lcd_t *lcd, const char *str, size_t len)
{
    for (size_t i = 0; i < len; i++)
    {
        send_data(lcd, (uint8_t)str[i]);
        cmd_short_wait(lcd);
    }
}

/**
 * @brief Write a NUL-terminated string at the current cursor position.
 *
 * @param lcd LCD instance
 * @param str NUL-terminated string to write
 */
void epic_lcd_print(epic_lcd_t *lcd, const char *str)
{
    epic_lcd_write(lcd, str, strlen(str));
}

/**
 * @brief Turn the entire display on or off.
 *
 * Cursor and blink settings are preserved; nothing is cleared.
 *
 * @param lcd LCD instance
 * @param on  true to turn the display on, false to turn it off
 */
void epic_lcd_display_on(epic_lcd_t *lcd, bool on)
{
    if (on)
    {
        lcd->display_ctrl |= DISPLAY_ON;
    }
    else
    {
        lcd->display_ctrl &= (uint8_t)~DISPLAY_ON;
    }
    send_cmd(lcd, CMD_DISPLAY_CTRL | lcd->display_ctrl);
    cmd_short_wait(lcd);
}

/**
 * @brief Show or hide the cursor underline.
 *
 * @param lcd LCD instance
 * @param on  true to show the cursor, false to hide it
 */
void epic_lcd_cursor_on(epic_lcd_t *lcd, bool on)
{
    if (on)
    {
        lcd->display_ctrl |= DISPLAY_CURSOR;
    }
    else
    {
        lcd->display_ctrl &= (uint8_t)~DISPLAY_CURSOR;
    }
    send_cmd(lcd, CMD_DISPLAY_CTRL | lcd->display_ctrl);
    cmd_short_wait(lcd);
}

/**
 * @brief Enable or disable cursor blinking.
 *
 * @param lcd LCD instance
 * @param on  true to blink the cursor, false to stop blinking
 */
void epic_lcd_cursor_blink(epic_lcd_t *lcd, bool on)
{
    if (on)
    {
        lcd->display_ctrl |= DISPLAY_BLINK;
    }
    else
    {
        lcd->display_ctrl &= (uint8_t)~DISPLAY_BLINK;
    }
    send_cmd(lcd, CMD_DISPLAY_CTRL | lcd->display_ctrl);
    cmd_short_wait(lcd);
}

/**
 * @brief Shift the entire display one position to the left.
 *
 * The cursor does not move.
 *
 * @param lcd LCD instance
 */
void epic_lcd_scroll_left(epic_lcd_t *lcd)
{
    send_cmd(lcd, CMD_CURSOR_SHIFT | SHIFT_DISPLAY);
    cmd_short_wait(lcd);
}

/**
 * @brief Shift the entire display one position to the right.
 *
 * The cursor does not move.
 *
 * @param lcd LCD instance
 */
void epic_lcd_scroll_right(epic_lcd_t *lcd)
{
    send_cmd(lcd, CMD_CURSOR_SHIFT | SHIFT_DISPLAY | SHIFT_RIGHT);
    cmd_short_wait(lcd);
}

/**
 * @brief Define a custom character in CGRAM slot @p slot.
 *
 * Slot range is 0-7, mapped to character codes 0x00-0x07. Glyph: 8 bytes,
 * one per row, bottom 5 bits are the pixel row (bit 4 = leftmost pixel,
 * bit 0 = rightmost). After defining, the address counter returns to
 * DDRAM.
 *
 * @param lcd   LCD instance
 * @param slot  CGRAM slot to define (0-7); out-of-range is ignored
 * @param glyph 8-byte glyph pattern, one byte per row
 */
void epic_lcd_create_char(epic_lcd_t *lcd, uint8_t slot, const uint8_t glyph[8])
{
    if (slot > 7u)
    {
        return;
    }
    send_cmd(lcd, CMD_SET_CGRAM_ADDR | (uint8_t)(slot << 3u));
    cmd_short_wait(lcd);
    for (uint8_t i = 0; i < 8u; i++)
    {
        send_data(lcd, glyph[i] & 0x1Fu);
        cmd_short_wait(lcd);
    }
    /* Return to DDRAM so subsequent writes go to the display, not CGRAM */
    send_cmd(lcd, CMD_SET_DDRAM_ADDR);
    cmd_short_wait(lcd);
}

/**
 * @brief Send a raw instruction byte.
 *
 * For commands not covered by the API above.
 *
 * @param lcd LCD instance
 * @param cmd raw HD44780 instruction byte
 */
void epic_lcd_command(epic_lcd_t *lcd, uint8_t cmd)
{
    send_cmd(lcd, cmd);
    cmd_short_wait(lcd);
}

/**
 * @brief Send a raw data byte.
 *
 * Writes to DDRAM/CGRAM at the current address.
 *
 * @param lcd  LCD instance
 * @param data data byte to write
 */
void epic_lcd_data(epic_lcd_t *lcd, uint8_t data)
{
    send_data(lcd, data);
    cmd_short_wait(lcd);
}
