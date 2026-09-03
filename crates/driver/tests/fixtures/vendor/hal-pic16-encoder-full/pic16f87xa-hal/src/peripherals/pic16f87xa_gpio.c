/* GPIO driver implementation (DS39582B §4.1..§4.5). */

#include "peripherals/pic16f87xa_gpio.h"
#include "core/pic16_irq.h"

/**
 * @brief  Map a GPIO_TypeDef to the address of its TRIS register.
 *         TRISx is always at PORTx | 0x80 in the banked register file
 *         (DS39582B Figure 2-3 / 2-4).
 * @param port GPIOA..GPIOE.
 * @return the TRISx SFR address.
 */
static uint8_t tris_addr(GPIO_TypeDef port)
{
    switch (port) {
        case GPIOA: return PIC_REG_TRISA;
        case GPIOB: return PIC_REG_TRISB;
        case GPIOC: return PIC_REG_TRISC;
#if PIC16F87XA_FAMILY_HAS_PORTD
        case GPIOD: return PIC_REG_TRISD;
#endif
#if PIC16F87XA_FAMILY_HAS_PORTE
        case GPIOE: return PIC_REG_TRISE;
#endif
        default:    return PIC_REG_TRISA;
    }
}

/**
 * @brief Map a GPIO_TypeDef to the PORTx register address.
 * @param port GPIOA..GPIOE.
 * @return the PORTx SFR address.
 */
static uint8_t port_addr(GPIO_TypeDef port)
{
    switch (port) {
        case GPIOA: return PIC_REG_PORTA;
        case GPIOB: return PIC_REG_PORTB;
        case GPIOC: return PIC_REG_PORTC;
#if PIC16F87XA_FAMILY_HAS_PORTD
        case GPIOD: return PIC_REG_PORTD;
#endif
#if PIC16F87XA_FAMILY_HAS_PORTE
        case GPIOE: return PIC_REG_PORTE;
#endif
        default:    return PIC_REG_PORTA;
    }
}

/**
 * @brief  Upper pin bound for a port. PORTA = 6, PORTE = 3, others = 8.
 *         Anything above this is unimplemented (DS39582B Table 4-1..4-9,
 *         marked with a dash in the datasheet).
 * @param port GPIOA..GPIOE.
 * @return the number of implemented pins.
 */
static uint8_t port_width(GPIO_TypeDef port)
{
#if PIC16F87XA_FAMILY_HAS_PORTE
    if (port == GPIOE) return 3U;
#endif
    if (port == GPIOA) return 6U;
    return 8U;
}

/* init / deinit. */

/**
 * @brief Configure one or more pins of a port as input, output or analog.
 * @param port GPIOA..GPIOE.
 * @param pins bitmask of pins to configure.
 * @param mode GPIO_MODE_INPUT, GPIO_MODE_OUTPUT or GPIO_MODE_ANALOG.
 */
void EPIC_GPIO_Init(GPIO_TypeDef port, uint16_t pins, GPIO_ModeTypeDef mode)
{
    uint8_t ta = tris_addr(port);
    uint8_t mask   = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);

    uint8_t tris = EPIC_REG8(ta);

    switch (mode) {
        case GPIO_MODE_INPUT:
        case GPIO_MODE_ANALOG:
            /* Both modes set TRIS=1 (input). Analog mode additionally
             * requires ADCON1 configuration, which the user does separately
             * via EPIC_ADC_ConfigChannels(). */
            tris |= mask;
            break;
        case GPIO_MODE_OUTPUT:
            tris &= (uint8_t)~mask;
            break;
        default:
            return;
    }
    EPIC_REG8(ta) = tris;
}

/**
 * @brief Restore all pins of a port to input mode.
 * @param port GPIOA..GPIOE.
 */
void EPIC_GPIO_DeInit(GPIO_TypeDef port)
{
    uint8_t ta = tris_addr(port);
    /* Reset all implemented bits of TRISx to 1 = input. */
    EPIC_REG8(ta) = (uint8_t)((1U << port_width(port)) - 1U);
}

/* read / write / toggle. */

/**
 * @brief Drive a set of pins high or low.
 * @param port GPIOA..GPIOE.
 * @param pins bitmask of pins to drive.
 * @param state GPIO_PIN_SET or GPIO_PIN_RESET.
 */
void EPIC_GPIO_WritePin(GPIO_TypeDef port, uint16_t pins, GPIO_PinState state)
{
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);
    uint8_t pa = port_addr(port);
    uint8_t cur = EPIC_REG8(pa);
    if (state == GPIO_PIN_SET) cur |= mask;
    else                       cur &= (uint8_t)~mask;
    EPIC_REG8(pa) = cur;
}

/**
 * @brief Invert a set of pins.
 * @param port GPIOA..GPIOE.
 * @param pins bitmask of pins to toggle.
 */
void EPIC_GPIO_TogglePin(GPIO_TypeDef port, uint16_t pins)
{
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);
    uint8_t pa = port_addr(port);
    EPIC_REG8(pa) = EPIC_REG8(pa) ^ mask;
}

/**
 * @brief Read the current level of a set of pins.
 * @param port GPIOA..GPIOE.
 * @param pins bitmask of pins to sample.
 * @return GPIO_PIN_SET if any sampled pin is high, else GPIO_PIN_RESET.
 */
GPIO_PinState EPIC_GPIO_ReadPin(GPIO_TypeDef port, uint16_t pins)
{
    /* TRIS=1 (input) returns the pin state; TRIS=0 (output) the latch.
     * The sim backend implements the same behavior; XC8 lowers this to
     * a single MOVF on the real target. */
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);
    uint8_t pa = port_addr(port);
    return (EPIC_REG8(pa) & mask) ? GPIO_PIN_SET : GPIO_PIN_RESET;
}

/**
 * @brief Write the whole port latch.
 * @param port GPIOA..GPIOE.
 * @param value the byte to write (unimplemented bits are masked off).
 */
void EPIC_GPIO_WritePort(GPIO_TypeDef port, uint8_t value)
{
    uint8_t mask = (uint8_t)((1U << port_width(port)) - 1U);
    EPIC_REG8(port_addr(port)) = (uint8_t)(value & mask);
}

/**
 * @brief Read the whole port latch.
 * @param port GPIOA..GPIOE.
 * @return the port byte.
 */
uint8_t EPIC_GPIO_ReadPort(GPIO_TypeDef port)
{
    return EPIC_REG8(port_addr(port));
}

/* PORTB pull-ups. */

/**
 * @brief Enable or disable the PORTB internal weak pull-ups.
 * @param pull GPIO_PULLUP or GPIO_NOPULL.
 */
void EPIC_GPIO_SetPullups(GPIO_PullTypeDef pull)
{
#ifdef EPIC_BANK1_READ8
    /* Plain EPIC_REG8 RMWs on the Bank-1 OPTION_REG silently misdirect
     * to the Bank-0 alias (TMR0) under XC8 v4.00; see the probe note
     * in pic16f87xa_timer0.c's option_clr_set. */
    uint8_t opt = 0u;
    EPIC_BANK1_READ8(OPTION_REG, opt);
    if (pull == GPIO_PULLUP) {
        opt &= (uint8_t)0x7F;    /* RBPU = 0 → enabled */
    } else {
        opt |= (uint8_t)0x80;    /* RBPU = 1 → disabled */
    }
    EPIC_BANK1_WRITE8(OPTION_REG, opt);
#else
    uint8_t opt = EPIC_REG8(PIC_REG_OPTION);
    if (pull == GPIO_PULLUP) {
        EPIC_BIT_CLR(opt, (uint8_t)0x80);    /* RBPU = 0 → enabled */
    } else {
        EPIC_BIT_SET(opt, (uint8_t)0x80);    /* RBPU = 1 → disabled */
    }
    EPIC_REG8(PIC_REG_OPTION) = opt;
#endif
}

/* PORTB change interrupt. */

/* One callback slot for the whole-port RB<7:4> change interrupt. There is
 * only one PORTB, so there is no handle struct (mirrors Timer2's
 * one-callback-per-handle shape but simpler). NULL = unregistered/no-op. */
static void (*s_rb_change_callback)(uint8_t) = NULL;

/**
 * @brief Install or remove the PORTB change callback.
 * @param callback function called with the PORTB byte on an RB<7:4>
 *        change, or NULL to unregister.
 */
void EPIC_GPIO_RegisterChangeCallback(void (*callback)(uint8_t))
{
    s_rb_change_callback = callback;
}

/**
 * @brief Weak RB<7:4> change ISR: reads PORTB first, clears RBIF, then
 *        fires the registered callback with the byte.
 */
void RB_IRQHandler(void)
{
    /* Direct flag ops (class-F: the table route clobbers PCLATH in ISR
     * context; see the CCP handlers). RBIF is INTCON bit 0
     * (bank-independent). */
    if (!(EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_RBIF)) return;

    /* MUST read PORTB before clearing RBIF (DS39582B §14.11.3): the
     * mismatch comparator only re-arms once PORTB is read, so reading
     * after clearing risks a spurious re-interrupt or a missed change. */
    uint8_t portb = EPIC_REG8(PIC_REG_PORTB);
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_RBIF);
    if (s_rb_change_callback) s_rb_change_callback(portb);
}
