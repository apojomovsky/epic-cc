/*
 * Menu demo core: LCD 3-screen control panel, ADC live reading, an
 * EEPROM-persisted brightness setting driving a CCP1 PWM output, and a
 * UART heartbeat. All state lives here so both entry points (real
 * target and sim) exercise the same logic; see menu_demo_core.h for the
 * event-queue seam that lets the sim inject synthetic button presses.
 */

#include "menu_demo_core.h"

#include "epic_hal.h"
#include "epic_lcd.h"
#include "epic_lcd_transport.h"
#include "epic_serial.h"
#include "epic_taskmgr.h"
#include "epic_tick.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 48000000UL
#endif

/* LCD: 4-bit GPIO transport on PORTD, RS/E/DB4-7 (16x2 HD44780). */
static const epic_lcd_gpio4_pins_t LCD_PINS = {
    .rs_port  = GPIOD, .rs_pin  = GPIO_PIN_0,
    .e_port   = GPIOD, .e_pin   = GPIO_PIN_1,
    .db4_port = GPIOD, .db4_pin = GPIO_PIN_4,
    .db5_port = GPIOD, .db5_pin = GPIO_PIN_5,
    .db6_port = GPIOD, .db6_pin = GPIO_PIN_6,
    .db7_port = GPIOD, .db7_pin = GPIO_PIN_7,
};

/* Buttons: RB4/RB5/RB6, inside the RB<7:4> change-interrupt group. */
#define BTN_UP_PIN     GPIO_PIN_4
#define BTN_DOWN_PIN   GPIO_PIN_5
#define BTN_SELECT_PIN GPIO_PIN_6

/* EEPROM layout: one magic byte (first-boot detection) + one setting
 * byte. 0xE9 is arbitrary, chosen to not collide with an erased byte
 * (0xFF) or a zeroed one (0x00). */
#define EE_ADDR_MAGIC      0x00U
#define EE_ADDR_BRIGHTNESS 0x01U
#define EE_MAGIC_VALUE     0xE9U

#define BRIGHTNESS_MAX 10U

/* Button event ring. A real button press is drained well before the next
 * one arrives, but the sim stimulus (tests/sim_menu_demo.c) can catch up
 * several scripted ticks' worth of events in a single scheduler round
 * (a real MPLAB SIM characteristic: a round that does LCD/ADC/EEPROM
 * work costs enough simulated time that Timer0 can advance several
 * ticks between rounds), so this needs headroom for a multi-event
 * burst, not just one in flight. 8 (7 usable slots) comfortably covers
 * the sim script's worst observed burst (4 events in one round). */
#define EVENT_RING_SZ 8U

static volatile menu_event_t g_event_ring[EVENT_RING_SZ];
static volatile uint8_t      g_event_head;
static volatile uint8_t      g_event_tail;

static epic_lcd_t   g_lcd;
static menu_screen_t g_screen = MENU_SCREEN_STATUS;

static uint8_t  g_brightness;
static uint8_t  g_brightness_dirty;
typedef enum { EE_IDLE = 0, EE_WRITING_MAGIC, EE_WRITING_BRIGHTNESS } ee_state_t;
static ee_state_t g_ee_state = EE_IDLE;
static uint16_t g_eeprom_writes;

static uint16_t g_adc_value;

/* ------------------------------------------------------------------ */
/* Event queue                                                         */
/* ------------------------------------------------------------------ */

/**
 * @brief Queue a button event (small ring, drops silently if full).
 * @param ev the event to queue
 */
void menu_demo_push_event(menu_event_t ev)
{
    uint8_t next = (uint8_t)((g_event_head + 1U) % EVENT_RING_SZ);
    if (next == g_event_tail)
    {
        return; /* ring full: drop, same policy as a debounced button */
    }
    g_event_ring[g_event_head] = ev;
    g_event_head = next;
}

/**
 * @brief Pop one event from the ring, if any.
 * @param out receives the popped event when non-empty
 * @return 1 if an event was popped, 0 if the ring was empty
 */
static int event_pop(menu_event_t *out)
{
    if (g_event_tail == g_event_head)
    {
        return 0;
    }
    *out = g_event_ring[g_event_tail];
    g_event_tail = (uint8_t)((g_event_tail + 1U) % EVENT_RING_SZ);
    return 1;
}

/* ------------------------------------------------------------------ */
/* LCD rendering                                                       */
/* ------------------------------------------------------------------ */

#define LCD_COLS 16u

/* One shared line buffer: every redraw_* helper below builds a line here
 * (prefix + space-fill, optionally a right-justified number in the
 * tail) then writes it out, so every write is a full-width overwrite
 * of the row -- no epic_lcd_clear() needed (its ~1.53 ms command would
 * need epic_tick running continuously; see menu_demo_init). */
static char g_line_buf[LCD_COLS + 1u];

/* Fill g_line_buf with a literal prefix, then spaces out to LCD_COLS,
 * NUL-terminated. A macro, not a shared function taking (pointer,
 * length): a real function called from several sites with differently
 * sized literals is exactly the shape cppcheck's interprocedural
 * analysis cannot correlate (it flags a possible out-of-bounds read
 * using one call site's array size against another's bound), so each
 * expansion here gets its own literal's compile-time size instead. */
#define BUILD_LINE(prefix_literal) do {                               \
    static const char epic_menu_lit_[] = (prefix_literal);            \
    uint8_t epic_menu_i_;                                              \
    for (epic_menu_i_ = 0;                                            \
         epic_menu_i_ < LCD_COLS &&                                    \
         epic_menu_i_ < (uint8_t)(sizeof(epic_menu_lit_) - 1u);        \
         epic_menu_i_++)                                               \
    {                                                                  \
        g_line_buf[epic_menu_i_] = epic_menu_lit_[epic_menu_i_];       \
    }                                                                  \
    for (; epic_menu_i_ < LCD_COLS; epic_menu_i_++)                    \
    {                                                                  \
        g_line_buf[epic_menu_i_] = ' ';                                \
    }                                                                  \
    g_line_buf[LCD_COLS] = '\0';                                       \
} while (0)

/**
 * @brief Right-justify v into the last `width` columns of g_line_buf.
 * @param v     value to render, in decimal
 * @param width number of trailing columns to fill
 */
static void put_u16_tail(uint16_t v, uint8_t width)
{
    uint8_t i;
    uint8_t end = LCD_COLS;
    for (i = 0; i < width; i++)
    {
        g_line_buf[end - 1U - i] = (char)('0' + (v % 10U));
        v /= 10U;
        if (v == 0U && i + 1U < width)
        {
            uint8_t j;
            for (j = (uint8_t)(i + 1U); j < width; j++)
            {
                g_line_buf[end - 1U - j] = ' ';
            }
            break;
        }
    }
}

/**
 * @brief Write g_line_buf to one LCD row.
 * @param row target row, 0 or 1
 */
static void lcd_write_line(uint8_t row)
{
    epic_lcd_set_cursor(&g_lcd, 0U, row);
    epic_lcd_print(&g_lcd, g_line_buf);
}

/** @brief Redraw the STATUS screen (tick count + ADC reading). */
static void redraw_status(void)
{
    BUILD_LINE("Ticks:");
    put_u16_tail(epic_taskmgr_ticks(), 5U);
    lcd_write_line(0U);
    BUILD_LINE("ADC:");
    put_u16_tail(g_adc_value, 4U);
    lcd_write_line(1U);
}

/** @brief Redraw the BRIGHTNESS screen (label + a 10-segment bar). */
static void redraw_brightness(void)
{
    uint8_t i;
    BUILD_LINE("Brightness:");
    lcd_write_line(0U);
    BUILD_LINE("[          ]");
    for (i = 0; i < 10U; i++)
    {
        g_line_buf[1U + i] = (i < g_brightness) ? '#' : '.';
    }
    lcd_write_line(1U);
}

/** @brief Redraw the ABOUT screen (name + EEPROM write count). */
static void redraw_about(void)
{
    BUILD_LINE("epic-menu-demo");
    lcd_write_line(0U);
    BUILD_LINE("EEwr:");
    put_u16_tail(g_eeprom_writes, 5U);
    lcd_write_line(1U);
}

/** @brief Redraw whichever screen is currently active. */
static void redraw(void)
{
    switch (g_screen)
    {
    case MENU_SCREEN_STATUS:     redraw_status();     break;
    case MENU_SCREEN_BRIGHTNESS: redraw_brightness();  break;
    case MENU_SCREEN_ABOUT:      redraw_about();       break;
    default: break;
    }
}

/* ------------------------------------------------------------------ */
/* Init                                                                 */
/* ------------------------------------------------------------------ */

/** @brief Initialize the LCD, ADC, EEPROM-backed settings, and PWM. */
void menu_demo_init(void)
{
    /* static, not a stack local: epic_lcd_init stores &ops into global
     * g_lcd, dereferenced by every later epic_lcd_* call, long after
     * this function returns (a stack local here would dangle -- see
     * docs/pic18f4550-menu-demo.md). ops_ctx stays a plain local: only
     * its value, not its address, ever gets stored anywhere. */
    static epic_lcd_ops_t ops;
    void *ops_ctx;
    /* Field-by-field, not a partial `{ ..., .row_addr = {0U} }`
     * aggregate literal: clang lowers the array's implied zero-fill to
     * an `llvm.memset` intrinsic, which epic-cc's legalize pass does
     * not yet support without a materialized destination address
     * (epic-cc issue TBD). Unrolled scalar stores sidestep it. */
    epic_lcd_config_t cfg;
    cfg.cols = 16U;
    cfg.rows = 2U;
    cfg.row_addr[0] = 0U;
    cfg.row_addr[1] = 0U;
    cfg.row_addr[2] = 0U;
    cfg.row_addr[3] = 0U;

    epic_serial_init(FOSC_HZ, 9600U);

    EPIC_GPIO_Init(GPIOB, BTN_UP_PIN | BTN_DOWN_PIN | BTN_SELECT_PIN, GPIO_MODE_INPUT);
    EPIC_GPIO_SetPullups(GPIO_PULLUP);

    /* epic-tick (Timer2-based) drives the LCD transport's startup
     * delays (epic_lcd_gpio4.c: gpio4_delay_ms -> epic_tick_delay_ms);
     * epic_tick_init enables GIE itself, so its ISR advances the tick
     * during the blocking init delays below. Timer2 is reclaimed for
     * the CCP1 PWM time base right after LCD init completes (PIC18 PWM
     * is hard-wired to Timer2, so the two uses cannot coexist);
     * nothing after this point issues a >=1 ms LCD delay (redraw()
     * never calls epic_lcd_clear(), see its comment). */
    epic_tick_init(FOSC_HZ);

    epic_lcd_gpio4_init(&ops, &ops_ctx, &LCD_PINS);
    epic_lcd_init(&g_lcd, &ops, ops_ctx, &cfg);

    ADC_HandleTypeDef adc = ADC_HANDLE_DEFAULT;
    adc.Channel     = ADC_CHANNEL_AN0;
    adc.PinConfig   = 0x0U; /* all AN pins analog; only AN0 wired */
    EPIC_ADC_Init(&adc);

    EPIC_GPIO_Init(GPIOC, GPIO_PIN_2, GPIO_MODE_OUTPUT); /* CCP1/P1A = RC2 */
    TIMER2_HandleTypeDef t2 = TIMER2_HANDLE_DEFAULT;
    t2.Prescaler = TIMER2_PRESCALER_1_16;
    t2.Period    = 255U;
    EPIC_TIMER2_Init(&t2);
    EPIC_TIMER2_Start(&t2);

    /* Field-by-field, not `= { 0 }`: see the cfg comment above for why. */
    CCP_HandleTypeDef ccp;
    ccp.Instance             = CCP_INSTANCE_1;
    ccp.Mode                 = CCP_MODE_PWM;
    ccp.CompareValue         = 0U;
    ccp.PWM.Period           = 255U;
    ccp.PWM.Duty             = 0U;
    ccp.PWMOutputMode        = CCP_PWM_OUTPUT_SINGLE;
    ccp.DeadBand.Delay       = 0U;
    ccp.DeadBand.AutoRestart = false;
    ccp.AutoShutdown.Source  = CCP_AUTOSHUTDOWN_DISABLED;
    ccp.AutoShutdown.PinsAC  = CCP_SHUTDOWN_DRIVE_0;
    ccp.AutoShutdown.PinsBD  = CCP_SHUTDOWN_DRIVE_0;
    ccp.EventCallback        = NULL;
    EPIC_CCP_Init(&ccp);

    uint8_t magic = EPIC_EEPROM_ReadByte(EE_ADDR_MAGIC);
    if (magic == EE_MAGIC_VALUE)
    {
        g_brightness = EPIC_EEPROM_ReadByte(EE_ADDR_BRIGHTNESS);
        if (g_brightness > BRIGHTNESS_MAX)
        {
            g_brightness = BRIGHTNESS_MAX;
        }
    }
    else
    {
        g_brightness = 5U; /* default on first boot / blank EEPROM */
        g_brightness_dirty = 1U;
    }

    g_screen = MENU_SCREEN_STATUS;
    redraw();
}

/* ------------------------------------------------------------------ */
/* taskmgr tasks                                                       */
/* ------------------------------------------------------------------ */

/**
 * @brief taskmgr task: sample the ADC, redraw if on the status screen.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_adc(void *arg)
{
    (void)arg;
    EPIC_ADC_SelectChannel(ADC_CHANNEL_AN0);
    (void)EPIC_ADC_Start();
    while (EPIC_ADC_IsConversionInProgress())
    {
        /* Short, bounded: acquisition + conversion is a handful of Tad
         * (~ microseconds), far shorter than the task period. */
    }
    g_adc_value = EPIC_ADC_Read();
    EPIC_ADC_ClearITFlag();

    if (g_screen == MENU_SCREEN_STATUS)
    {
        redraw_status();
    }
}

/**
 * @brief taskmgr task: drain queued button events, update state, redraw.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_ui(void *arg)
{
    menu_event_t ev;
    uint8_t changed = 0U;

    (void)arg;
    while (event_pop(&ev))
    {
        if (ev == MENU_EVENT_SELECT)
        {
            g_screen = (menu_screen_t)((g_screen + 1U) % MENU_SCREEN_COUNT);
            changed = 1U;
        }
        else if (g_screen == MENU_SCREEN_BRIGHTNESS)
        {
            if (ev == MENU_EVENT_UP && g_brightness < BRIGHTNESS_MAX)
            {
                g_brightness++;
                g_brightness_dirty = 1U;
                changed = 1U;
            }
            else if (ev == MENU_EVENT_DOWN && g_brightness > 0U)
            {
                g_brightness--;
                g_brightness_dirty = 1U;
                changed = 1U;
            }
        }
    }

    if (changed)
    {
        redraw();
    }
}

/**
 * @brief taskmgr task: autosave the brightness setting when dirty.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_eeprom(void *arg)
{
    (void)arg;
    switch (g_ee_state)
    {
    case EE_IDLE:
        if (g_brightness_dirty)
        {
            (void)EPIC_EEPROM_WriteByte(EE_ADDR_MAGIC, EE_MAGIC_VALUE);
            g_ee_state = EE_WRITING_MAGIC;
        }
        break;
    case EE_WRITING_MAGIC:
        if (EPIC_EEPROM_IsWriteComplete())
        {
            g_eeprom_writes++;
            (void)EPIC_EEPROM_WriteByte(EE_ADDR_BRIGHTNESS, g_brightness);
            g_ee_state = EE_WRITING_BRIGHTNESS;
        }
        break;
    case EE_WRITING_BRIGHTNESS:
        if (EPIC_EEPROM_IsWriteComplete())
        {
            g_eeprom_writes++;
            g_brightness_dirty = 0U;
            g_ee_state = EE_IDLE;
        }
        break;
    default:
        g_ee_state = EE_IDLE;
        break;
    }
}

/**
 * @brief taskmgr task: UART heartbeat line + PWM duty update.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_heartbeat(void *arg)
{
    (void)arg;
    EPIC_CCP_SetPWMDuty(CCP_INSTANCE_1, (uint16_t)g_brightness * 100U);

    epic_serial_put_str("HB scr=");
    epic_serial_put_u16((uint16_t)g_screen);
    epic_serial_put_str(" br=");
    epic_serial_put_u16((uint16_t)g_brightness);
    epic_serial_put_str(" adc=");
    epic_serial_put_u16(g_adc_value);
    epic_serial_put_str(" we=");
    epic_serial_put_u16(g_eeprom_writes);
    epic_serial_put_str(" t=");
    epic_serial_put_u16(epic_taskmgr_ticks());
    epic_serial_put_str(" eest=");
    epic_serial_put_u16((uint16_t)g_ee_state);
    epic_serial_put_str("\n");
}

/* ------------------------------------------------------------------ */
/* Introspection                                                       */
/* ------------------------------------------------------------------ */

/**
 * @brief Latest ADC reading, for the sim oracle's cross-checks.
 * @return the last sampled ADC value, 0..1023
 */
uint16_t menu_demo_adc_value(void)
{
    return g_adc_value;
}

/**
 * @brief Current brightness setting, for the sim oracle's cross-checks.
 * @return the brightness setting, 0..10
 */
uint8_t menu_demo_brightness(void)
{
    return g_brightness;
}

/**
 * @brief Current menu screen, for the sim oracle's cross-checks.
 * @return the active screen
 */
menu_screen_t menu_demo_screen(void)
{
    return g_screen;
}

/**
 * @brief Completed EEPROM write count, for the sim oracle's cross-checks.
 * @return the number of completed EEPROM byte writes
 */
uint16_t menu_demo_eeprom_writes(void)
{
    return g_eeprom_writes;
}
