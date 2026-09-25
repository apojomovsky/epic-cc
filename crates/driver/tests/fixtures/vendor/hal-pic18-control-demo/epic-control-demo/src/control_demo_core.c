/*
 * Control demo core: console driven PID loop over ADC AN0 and CCP1 PWM,
 * with an EEPROM-persisted settings struct. All state lives here so both
 * entry points (real target and sim) exercise the same logic; see
 * control_demo_core.h for the injection seam that lets the sim feed
 * scripted console bytes and ADC values.
 */

#include "control_demo_core.h"

#include "epic_hal.h"
#include "epic_adcfilter.h"
#include "epic_serial.h"
#include "epic_taskmgr.h"
#include "pid.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 48000000UL
#endif

/* Oversampling adds two effective bits, so the loop regulates a 12-bit
 * value (one raw 10-bit reading times four) and the setpoint uses the
 * same domain. Sixteen conversions per step is microseconds of Tad,
 * far shorter than the control period. */
#define OVERSAMPLE_BITS 2U
#define AVG_WINDOW      4U
#define PWM_FULL        1000U
#define SETPOINT_MAX    4095U

/* EEPROM image: magic, setpoint lo/hi, kp lo/hi, ki lo/hi, kd lo/hi,
 * crc. 0xC7 is arbitrary, chosen to differ from an erased cell (0xFF)
 * and a zeroed one (0x00), the same reason the menu demo uses 0xE9. */
#define EE_ADDR_BASE 0x00U
#define EE_IMAGE_SZ  10U
#define EE_MAGIC_VALUE 0xC7U

#define CONSOLE_LINE_SZ 32U
#define RX_DRAIN_SZ     8U

typedef enum { EE_IDLE = 0, EE_WRITING_MAGIC, EE_WRITING_BODY } ee_state_t;

/* Single-store settings: g_ee_image is both the live store and the
 * EEPROM snapshot, so the two can never skew. A console write landing
 * mid-save tears the in-flight image; g_ee_resave then forces a second
 * pass once the torn one completes, converging on the stable bytes. */
static uint8_t g_ee_image[EE_IMAGE_SZ];

/* One control block instead of scattered tiny globals: the
 * linker's small-data pools (access RAM holding epic-math's
 * absolute-addressed asm temporaries) are nearly full on this part, so
 * a single multi-byte symbol packs into a banked pool and leaves that
 * room for symbols that cannot move. */
static struct {
    ee_state_t ee_state;
    uint8_t    ee_index;
    uint8_t    ee_resave;
    uint8_t    dirty;
    uint8_t    sim_valid;
    uint8_t    line_len;
} g_s;
static uint16_t g_eeprom_writes;
static epic_pid_t           g_pid;
static epic_adcfilter_avg_t g_avg;
static uint16_t             g_avg_buf[AVG_WINDOW];

static uint16_t g_measurement;
static int16_t  g_output;

/* Sim plant override: set by control_demo_inject_adc, never on real
 * hardware, so the sim samples scripted values with no conversions. */
static uint16_t g_sim_adc;

static char g_line[CONSOLE_LINE_SZ];

static uint8_t s_rx_drain[RX_DRAIN_SZ];

/**
 * @brief Compare two NUL-terminated strings for equality.
 * @param a first string
 * @param b second string
 * @return 1 when equal, 0 otherwise
 */
static uint8_t line_eq(const char *a, const char *b)
{
    while (*a != '\0' && *a == *b)
    {
        a++;
        b++;
    }
    return (*a == *b) ? 1U : 0U;
}

/**
 * @brief Test whether s starts with the NUL-terminated prefix.
 * @param s the string to test
 * @param prefix the prefix to look for
 * @return 1 when s starts with prefix, 0 otherwise
 */
static uint8_t starts_with(const char *s, const char *prefix)
{
    while (*prefix != '\0')
    {
        if (*s != *prefix)
        {
            return 0U;
        }
        s++;
        prefix++;
    }
    return 1U;
}

/**
 * @brief Parse a signed decimal integer by hand (no sscanf).
 * @param s NUL-terminated input, optional leading minus plus digits
 * @param out receives the value on success
 * @return 1 on success, 0 on empty input, trailing garbage, or int16 overflow
 */
static uint8_t parse_i16(const char *s, int16_t *out)
{
    /* One flags byte (sign in bit 7, digit count below): this parser
     * sits on the console task's worst-case stack path, so it keeps no
     * wider autos than the digit math needs. */
    uint8_t  flags = 0U;
    uint16_t acc = 0U;
    uint16_t limit;
    uint8_t  d;
    if (*s == '-')
    {
        flags = 0x80U;
        s++;
    }
    limit = (flags != 0U) ? 32768U : 32767U;
    while (*s >= '0' && *s <= '9')
    {
        d = (uint8_t)(*s - '0');
        if (acc > limit / 10U || (acc == limit / 10U && d > limit % 10U))
        {
            return 0U;
        }
        acc = (uint16_t)(acc * 10U + d);
        flags++;
        s++;
    }
    if ((flags & 0x7FU) == 0U || *s != '\0')
    {
        return 0U;
    }
    if (flags & 0x80U)
    {
        *out = (int16_t)(0U - acc);
    }
    else
    {
        *out = (int16_t)acc;
    }
    return 1U;
}

/**
 * @brief Read one little-endian int16 field out of the image.
 * @param lo index of the low byte (high byte follows)
 * @return the decoded value
 */
static int16_t settings_get(uint8_t lo)
{
    uint16_t u = (uint16_t)g_ee_image[lo] | ((uint16_t)g_ee_image[lo + 1U] << 8);
    return (int16_t)u;
}

/**
 * @brief Write one little-endian int16 field into the image.
 * @param lo index of the low byte (high byte follows)
 * @param v the value to store
 */
static void settings_set(uint8_t lo, int16_t v)
{
    g_ee_image[lo] = (uint8_t)(v & 0xFF);
    g_ee_image[lo + 1U] = (uint8_t)((v >> 8) & 0xFF);
}

/**
 * @brief Mark the image dirty, requesting a re-save after any active one.
 */
static void settings_mark_dirty(void)
{
    g_s.dirty = 1U;
    if (g_s.ee_state != EE_IDLE)
    {
        g_s.ee_resave = 1U;
    }
}

/**
 * @brief Emit the full settings plus loop state as one text line.
 */
static void console_print_settings(void)
{
    epic_serial_put_str("sp=");
    epic_serial_put_i16(settings_get(1U));
    epic_serial_put_str(" kp=");
    epic_serial_put_i16(settings_get(3U));
    epic_serial_put_str(" ki=");
    epic_serial_put_i16(settings_get(5U));
    epic_serial_put_str(" kd=");
    epic_serial_put_i16(settings_get(7U));
    epic_serial_put_str(" meas=");
    epic_serial_put_u16(g_measurement);
    epic_serial_put_str(" out=");
    epic_serial_put_i16(g_output);
    epic_serial_put_str(" we=");
    epic_serial_put_u16(g_eeprom_writes);
    epic_serial_put_str("\n");
}

/**
 * @brief Apply one `set <name> <n>` command; args follows the "set " prefix.
 * @param args the name, one space, and the decimal value
 */
static void console_do_set(const char *args)
{
    int16_t v;
    uint8_t lo;
    if (starts_with(args, "sp "))
    {
        lo = 1U;
    }
    else if (starts_with(args, "kp "))
    {
        lo = 3U;
    }
    else if (starts_with(args, "ki "))
    {
        lo = 5U;
    }
    else if (starts_with(args, "kd "))
    {
        lo = 7U;
    }
    else
    {
        epic_serial_put_str("ERR set\n");
        return;
    }
    if (!parse_i16(args + 3, &v))
    {
        epic_serial_put_str("ERR set\n");
        return;
    }
    if (lo == 1U)
    {
        if (v < 0 || v > (int16_t)SETPOINT_MAX)
        {
            epic_serial_put_str("ERR range\n");
            return;
        }
    }
    settings_set(lo, v);
    if (lo != 1U)
    {
        epic_pid_set_gains(&g_pid, settings_get(3U), settings_get(5U), settings_get(7U));
    }
    settings_mark_dirty();
    epic_serial_put_str("OK\n");
}

/**
 * @brief Dispatch one completed console line to its command.
 */
static void console_dispatch(void)
{
    if (line_eq(g_line, "help"))
    {
        epic_serial_put_str("cmds: help get set save\n");
        epic_serial_put_str("set sp|kp|ki|kd <n>\n");
        return;
    }
    if (line_eq(g_line, "get"))
    {
        console_print_settings();
        return;
    }
    if (line_eq(g_line, "save"))
    {
        settings_mark_dirty();
        epic_serial_put_str("OK save\n");
        return;
    }
    if (starts_with(g_line, "set "))
    {
        console_do_set(g_line + 4);
        return;
    }
    epic_serial_put_str("ERR unknown\n");
}

/**
 * @brief Feed one byte into the console line buffer, dispatching on CR/LF.
 *
 * Non-printables are ignored and overlong lines are truncated in place,
 * so the parser holds no dynamic state and cannot overrun.
 *
 * @param b the byte to consume
 */
static void console_rx_byte(uint8_t b)
{
    if (b == '\r' || b == '\n')
    {
        if (g_s.line_len > 0U)
        {
            console_dispatch();
            g_s.line_len = 0U;
            g_line[0] = '\0';
        }
        return;
    }
    if (b < 0x20U || b > 0x7EU)
    {
        return;
    }
    if (g_s.line_len < CONSOLE_LINE_SZ - 1U)
    {
        g_line[g_s.line_len] = (char)b;
        g_s.line_len++;
        g_line[g_s.line_len] = '\0';
    }
}

/**
 * @brief Queue a console byte into the line parser (see control_demo_core.h).
 * @param b the byte to parse
 */
void control_demo_inject_console_byte(uint8_t b)
{
    console_rx_byte(b);
}

/**
 * @brief Override the ADC plant with a scripted value (see control_demo_core.h).
 * @param v the scripted raw ADC value, 0..1023
 */
void control_demo_inject_adc(uint16_t v)
{
    g_sim_adc = v;
    g_s.sim_valid = 1U;
}

/**
 * @brief Take one raw ADC sample, or the sim override when one was injected.
 * @param ctx unused (epic_adcfilter_read_fn signature)
 * @return one raw 10-bit sample
 */
static uint16_t adc_read_one(void *ctx)
{
    uint16_t v;
    (void)ctx;
    if (g_s.sim_valid)
    {
        return g_sim_adc;
    }
    EPIC_ADC_SelectChannel(ADC_CHANNEL_AN0);
    (void)EPIC_ADC_Start();
    while (EPIC_ADC_IsConversionInProgress())
    {
        /* Bounded by hardware: acquisition plus conversion is a handful
         * of Tad, far shorter than the control period. */
    }
    v = EPIC_ADC_Read();
    EPIC_ADC_ClearITFlag();
    return v;
}

/**
 * @brief Stamp the CRC over the live image bytes.
 *
 * Called once when a save starts. A console write landing mid-save
 * tears the stored bytes past this CRC, so the next boot would reject
 * them; g_ee_resave schedules a clean second pass first, so the torn
 * generation never survives long enough to matter.
 */
static void eeprom_stamp_crc(void)
{
    uint8_t i;
    uint8_t crc = 0U;
    for (i = 0U; i < EE_IMAGE_SZ - 1U; i++)
    {
        crc ^= g_ee_image[i];
    }
    g_ee_image[EE_IMAGE_SZ - 1U] = crc;
}

/**
 * @brief Load the settings from EEPROM, or install defaults when blank or corrupt.
 */
static void eeprom_load(void)
{
    uint8_t i;
    uint8_t crc = 0U;
    for (i = 0U; i < EE_IMAGE_SZ; i++)
    {
        g_ee_image[i] = EPIC_EEPROM_ReadByte((uint8_t)(EE_ADDR_BASE + i));
    }
    for (i = 0U; i < EE_IMAGE_SZ - 1U; i++)
    {
        crc ^= g_ee_image[i];
    }
    if (g_ee_image[0] != EE_MAGIC_VALUE || crc != g_ee_image[EE_IMAGE_SZ - 1U])
    {
        g_ee_image[0] = EE_MAGIC_VALUE;
        settings_set(1U, 2048);
        settings_set(3U, 128);
        settings_set(5U, 8);
        settings_set(7U, 0);
        settings_mark_dirty();
    }
}

/**
 * @brief Initialize AN0 for the control loop's sense input.
 *
 * Split from control_demo_init so the ADC handle's frame overlays the
 * PWM handles' below instead of stacking with them: the compiled stack
 * shares one bank with byte-addressed statics here, and every byte of
 * peak depth counts against them.
 */
static void adc_init(void)
{
    ADC_HandleTypeDef adc = ADC_HANDLE_DEFAULT;
    adc.Channel   = ADC_CHANNEL_AN0;
    adc.PinConfig = 0x0U; /* all AN pins analog; only AN0 wired */
    EPIC_ADC_Init(&adc);
}

/**
 * @brief Initialize Timer2 plus CCP1 for the control loop's PWM drive.
 *
 * Split from control_demo_init for the same frame-overlay reason as
 * adc_init above.
 */
static void pwm_init(void)
{
    TIMER2_HandleTypeDef t2 = TIMER2_HANDLE_DEFAULT;
    CCP_HandleTypeDef ccp;
    EPIC_GPIO_Init(GPIOC, GPIO_PIN_2, GPIO_MODE_OUTPUT); /* CCP1/P1A = RC2 */
    t2.Prescaler = TIMER2_PRESCALER_1_16;
    t2.Period    = 255U;
    EPIC_TIMER2_Init(&t2);
    EPIC_TIMER2_Start(&t2);

    /* Field-by-field, not `= { 0 }`: a partial aggregate literal lowers
     * its implied zero-fill to an llvm.memset intrinsic that epic-cc
     * cannot legalize without a materialized destination address, the
     * same gap the menu demo documents for its LCD and CCP handles. */
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
}

/**
 * @brief Initialize the serial console, ADC, EEPROM-backed settings, PID, and PWM.
 *
 * Call once before spawning any epic-taskmgr task below.
 */
void control_demo_init(void)
{
    epic_serial_init(FOSC_HZ, 9600U);

    adc_init();
    pwm_init();

    /* Static buffer outliving its filter: the average state keeps the
     * pointer for the life of the program, so a stack buffer here
     * would dangle on return. */
    epic_adcfilter_avg_init(&g_avg, g_avg_buf, AVG_WINDOW);

    eeprom_load();
    epic_pid_init(&g_pid, settings_get(3U), settings_get(5U), settings_get(7U),
                  0, (int16_t)PWM_FULL);
}

/**
 * @brief taskmgr task: sample AN0, filter, step the PID, drive PWM.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void control_demo_task_control(void *arg)
{
    uint16_t raw;
    (void)arg;
    raw = epic_adcfilter_oversample(adc_read_one, NULL, OVERSAMPLE_BITS);
    g_measurement = epic_adcfilter_avg_push(&g_avg, raw);
    g_output = epic_pid_update(&g_pid, settings_get(1U), (int16_t)g_measurement);
    EPIC_CCP_SetPWMDuty(CCP_INSTANCE_1, (uint16_t)g_output);
}

/**
 * @brief taskmgr task: drain the UART RX ring into the line parser.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void control_demo_task_console(void *arg)
{
    uint8_t i;
    int n;
    (void)arg;
    while (epic_serial_available() > 0)
    {
        n = epic_serial_read(s_rx_drain, (int)RX_DRAIN_SZ);
        if (n <= 0)
        {
            break;
        }
        for (i = 0U; i < (uint8_t)n; i++)
        {
            console_rx_byte(s_rx_drain[i]);
        }
    }
}

/**
 * @brief taskmgr task: autosave the settings struct when dirty.
 *
 * One self-timed byte per EEPROM state, the menu demo pattern extended
 * over the whole image: magic first, then the remaining bytes in order,
 * so an interrupted save can only ever leave a CRC mismatch that the
 * next boot rejects in favor of defaults.
 *
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void control_demo_task_eeprom(void *arg)
{
    (void)arg;
    switch (g_s.ee_state)
    {
    case EE_IDLE:
        if (g_s.dirty)
        {
            eeprom_stamp_crc();
            g_s.ee_resave = 0U;
            (void)EPIC_EEPROM_WriteByte(EE_ADDR_BASE, g_ee_image[0]);
            g_s.ee_state = EE_WRITING_MAGIC;
        }
        break;
    case EE_WRITING_MAGIC:
        if (EPIC_EEPROM_IsWriteComplete())
        {
            g_eeprom_writes++;
            g_s.ee_index = 1U;
            (void)EPIC_EEPROM_WriteByte((uint8_t)(EE_ADDR_BASE + g_s.ee_index),
                                        g_ee_image[g_s.ee_index]);
            g_s.ee_state = EE_WRITING_BODY;
        }
        break;
    case EE_WRITING_BODY:
        if (EPIC_EEPROM_IsWriteComplete())
        {
            g_eeprom_writes++;
            g_s.ee_index++;
            if (g_s.ee_index < EE_IMAGE_SZ)
            {
                (void)EPIC_EEPROM_WriteByte((uint8_t)(EE_ADDR_BASE + g_s.ee_index),
                                            g_ee_image[g_s.ee_index]);
            }
            else
            {
                g_s.ee_state = EE_IDLE;
                if (!g_s.ee_resave)
                {
                    g_s.dirty = 0U;
                }
            }
        }
        break;
    default:
        g_s.ee_state = EE_IDLE;
        break;
    }
}

/**
 * @brief taskmgr task: UART heartbeat line with loop state.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void control_demo_task_heartbeat(void *arg)
{
    (void)arg;
    epic_serial_put_str("HB sp=");
    epic_serial_put_i16(settings_get(1U));
    epic_serial_put_str(" meas=");
    epic_serial_put_u16(g_measurement);
    epic_serial_put_str(" out=");
    epic_serial_put_i16(g_output);
    epic_serial_put_str(" we=");
    epic_serial_put_u16(g_eeprom_writes);
    epic_serial_put_str(" t=");
    epic_serial_put_u16(epic_taskmgr_ticks());
    epic_serial_put_str("\n");
}

/**
 * @brief Filtered measurement in the 12-bit control domain, for the sim oracle.
 * @return the latest averaged measurement, 0..4092
 */
uint16_t control_demo_measurement(void)
{
    return g_measurement;
}

/**
 * @brief Latest PID output, for the sim oracle.
 * @return the last computed output, 0..1000
 */
int16_t control_demo_output(void)
{
    return g_output;
}

/**
 * @brief Current setpoint, for the sim oracle.
 * @return the setpoint in the 12-bit control domain
 */
int16_t control_demo_setpoint(void)
{
    return settings_get(1U);
}

/**
 * @brief Current proportional gain, for the sim oracle.
 * @return kp in Q8.8
 */
int16_t control_demo_gain_kp(void)
{
    return settings_get(3U);
}

/**
 * @brief Current integral gain, for the sim oracle.
 * @return ki in Q8.8, pre-multiplied by the control period
 */
int16_t control_demo_gain_ki(void)
{
    return settings_get(5U);
}

/**
 * @brief Current derivative gain, for the sim oracle.
 * @return kd in Q8.8, pre-divided by the control period
 */
int16_t control_demo_gain_kd(void)
{
    return settings_get(7U);
}

/**
 * @brief Completed EEPROM write count, for the sim oracle.
 * @return the number of completed EEPROM byte writes
 */
uint16_t control_demo_eeprom_writes(void)
{
    return g_eeprom_writes;
}
