/*
 * SFR address map for the PIC18F2455/2550/4455/4550 family (MVP subset).
 * Every address, bit mask, and reset value here is taken 1-to-1 from
 * DS39632E (Table 5-1 SFR Map, Registers 5-2/9-1/9-2/9-3/9-5/9-6/9-8/
 * 10-1/11-1), cross-checked against the PIC18Fxxxx DFP device header.
 * Macro naming (`PIC_REG_*` / `PIC_*_*`) matches `pic16f87xa_sfr.h` so
 * driver source reads identically across families. All of it is in the
 * Access Bank (0xF60-0xFFF), no BSR banking needed; PORTD/E/LATD/E/
 * TRISD/E exist only on 40/44-pin parts, gated by
 * `PIC18FXX5X_FAMILY_HAS_PORTD`/`HAS_PORTE`.
 */

#ifndef PIC18FXX5X_SFR_H
#define PIC18FXX5X_SFR_H

#include "pic18fxx5x.h"

/* Access Bank SFR addresses. */
/* DS39632E Table 5-1, SFR Map. All in the Access Bank (0xF60-0xFFF). */

/* Core CPU status, DS39632E Register 5-2. */
#define PIC_REG_STATUS        0xFD8U   /**< ALU status (N/OV/Z/DC/C).       */
#define PIC_REG_BSR           0xFE0U   /**< Bank Select Register (low nibble). */
#define PIC_REG_RCON          0xFD0U   /**< Reset Control (IPEN/TO/PD/POR/BOR). */

/* I/O ports, DS39632E §10.0, Table 10-1..10-5.
 * PORTx is the pin input sample; LATx is the output latch; TRISx is the
 * data-direction register. PIC18 exposes LATx as its own mapped register
 * (§10.0), so GPIO writes go through LATx, not PORTx. */
#define PIC_REG_PORTA         0xF80U
#define PIC_REG_PORTB         0xF81U
#define PIC_REG_PORTC         0xF82U
#if PIC18FXX5X_FAMILY_HAS_PORTD
#define PIC_REG_PORTD         0xF83U   /* 40/44-pin only. */
#endif
#if PIC18FXX5X_FAMILY_HAS_PORTE
#define PIC_REG_PORTE         0xF84U   /* 40/44-pin only. */
#endif

#define PIC_REG_LATA          0xF89U
#define PIC_REG_LATB          0xF8AU
#define PIC_REG_LATC          0xF8BU
#if PIC18FXX5X_FAMILY_HAS_PORTD
#define PIC_REG_LATD          0xF8CU   /* 40/44-pin only. */
#endif
#if PIC18FXX5X_FAMILY_HAS_PORTE
#define PIC_REG_LATE          0xF8DU   /* 40/44-pin only. */
#endif

#define PIC_REG_TRISA         0xF92U
#define PIC_REG_TRISB         0xF93U
#define PIC_REG_TRISC         0xF94U
#if PIC18FXX5X_FAMILY_HAS_PORTD
#define PIC_REG_TRISD         0xF95U   /* 40/44-pin only. */
#endif
#if PIC18FXX5X_FAMILY_HAS_PORTE
#define PIC_REG_TRISE         0xF96U   /* 40/44-pin only. */
#endif

/* Interrupt control, DS39632E §9.0, Register 9-1/9-2/9-3. */
#define PIC_REG_INTCON        0xFF2U   /**< GIE/PEIE/TMR0IE/INT0IE/RBIE + flags. */
#define PIC_REG_INTCON2       0xFF1U   /**< Pull-ups, INT edge, TMR0IP, RBIP.   */
#define PIC_REG_INTCON3       0xFF0U   /**< INT1/INT2 enable, flag, priority.   */

/* Peripheral interrupt flag/enable/priority, DS39632E Register 9-5/9-6/9-8. */
#define PIC_REG_PIR1          0xF9EU   /**< Peripheral Interrupt Flag Register 1. */
#define PIC_REG_PIE1          0xF9DU   /**< Peripheral Interrupt Enable Reg 1.    */
#define PIC_REG_IPR1          0xF9FU   /**< Peripheral Interrupt Priority Reg 1.  */
#define PIC_REG_PIR2          0xFA1U   /**< Peripheral Interrupt Flag Register 2 (TMR3IF, CCP2IF, ...). */
#define PIC_REG_PIE2          0xFA0U   /**< Peripheral Interrupt Enable Reg 2.    */
#define PIC_REG_IPR2          0xFA2U   /**< Peripheral Interrupt Priority Reg 2 (reset 0xFF, all high). */

/* Timer0, DS39632E §11.0, Register 11-1. */
#define PIC_REG_T0CON         0xFD5U   /**< Timer0 control (on/8-16bit/src/edge/PSA/PS). */
#define PIC_REG_TMR0L         0xFD6U   /**< Timer0 low byte (also 8-bit mode value). */
#define PIC_REG_TMR0H         0xFD7U   /**< Timer0 high byte (16-bit mode only).    */

/* Timer1, DS39632E §12.0, Register 12-1. 16-bit timer/counter. */
#define PIC_REG_T1CON         0xFCDU   /**< Timer1 control (RD16/run/prescale/osc/sync/cs/on). */
#define PIC_REG_TMR1L         0xFCEU   /**< Timer1 low byte.                          */
#define PIC_REG_TMR1H         0xFCFU   /**< Timer1 high byte.                         */

/* Timer2, DS39632E §12.0, Register 12-2. 8-bit timer with PR2 + postscaler. */
#define PIC_REG_T2CON         0xFCAU   /**< Timer2 control (postscaler/on/prescaler). */
#define PIC_REG_PR2           0xFCBU   /**< Timer2 period register (Access Bank).    */
#define PIC_REG_TMR2          0xFCCU   /**< Timer2 counter.                          */

/* Timer3, DS39632E §14.0, Register 14-1. 16-bit timer/counter (shares T1OSC). */
#define PIC_REG_T3CON         0xFB1U   /**< Timer3 control (RD16/CCP-sel/prescale/sync/cs/on). */
#define PIC_REG_TMR3L         0xFB2U   /**< Timer3 low byte.                          */
#define PIC_REG_TMR3H         0xFB3U   /**< Timer3 high byte.                         */

/* ECCP1 / CCP2, DS39632E §16.0. CCP1 is the Enhanced CCP (dead-band +
 * auto-shutdown, multi-output PWM); CCP2 is the plain CCP. */
#define PIC_REG_ECCP1AS       0xFB6U   /**< ECCP1 auto-shutdown control.              */
#define PIC_REG_ECCP1DEL      0xFB7U   /**< ECCP1 dead-band delay + restart enable.   */
#define PIC_REG_CCP2CON       0xFBAU   /**< CCP2 control (plain).                     */
#define PIC_REG_CCPR2L        0xFBBU   /**< CCP2 compare/capture/PWM duty low.        */
#define PIC_REG_CCPR2H        0xFBCU   /**< CCP2 compare/capture/PWM duty high.       */
#define PIC_REG_CCP1CON       0xFBDU   /**< ECCP1 control (P1M/DC1B/CCP1M).           */
#define PIC_REG_CCPR1L        0xFBEU   /**< ECCP1 compare/capture/PWM duty low.       */
#define PIC_REG_CCPR1H        0xFBFU   /**< ECCP1 compare/capture/PWM duty high.      */

/* MSSP, DS39632E §19.0 (SPI master/slave + I2C master/slave). */
#define PIC_REG_SSPCON2       0xFC5U   /**< MSSP control 2 (I2C master: SEN/PEN/etc). */
#define PIC_REG_SSPCON1       0xFC6U   /**< MSSP control 1 (mode/SSPEN/CKP/WCOL/SSPOV).*/
#define PIC_REG_SSPSTAT       0xFC7U   /**< MSSP status (SMP/CKE/D-A/P/S/R-W/UA/BF).  */
#define PIC_REG_SSPADD        0xFC8U   /**< MSSP address (I2C slave) / baud (master). */
#define PIC_REG_SSPBUF        0xFC9U   /**< MSSP data buffer.                          */

/* EUSART, DS39632E §20.0 (Enhanced USART). The EUSART adds BAUDCON (BRG16,
 * auto-baud, wake-up, sync-clock polarity) and the SPBRGH high byte that the
 * PIC16 plain USART lacks; TXSTA/RCSTA/TXREG/RCREG/SPBRG are the same shape
 * as the PIC16 USART. All in the Access Bank, addresses from the DFP
 * pic18f4550.h SFR map (cross-checked against DS39632E Table 5-1). */
#define PIC_REG_BAUDCON       0xFB8U   /**< EUSART baud-rate control (BRG16/ABDEN/WUE/...). */
#define PIC_REG_RCSTA         0xFABU   /**< EUSART receive control/status.            */
#define PIC_REG_TXSTA         0xFACU   /**< EUSART transmit control/status.            */
#define PIC_REG_TXREG         0xFADU   /**< EUSART transmit data register.             */
#define PIC_REG_RCREG         0xFAEU   /**< EUSART receive data register.              */
#define PIC_REG_SPBRG         0xFAFU   /**< EUSART baud-rate divisor, low byte.        */
#define PIC_REG_SPBRGH        0xFB0U   /**< EUSART baud-rate divisor, high byte (BRG16=1). */

/* Comparator, DS39632E §22.0 (two on-chip comparators). CMCON has the same
 * bit layout as the PIC16 CMCON; only the address differs (Access Bank).
 * CVRCON (comparator voltage reference) is at 0xFB5, ported separately. */
#define PIC_REG_CMCON         0xFB4U   /**< Comparator control (mode/inputs/outputs). */

/* Data EEPROM + Flash, DS39632E §7.0. EECON1 controls read/write; EECON2 is
 * the unlock register (0x55 then 0xAA); EEDATA/EEADR are the data/address
 * registers. The 2455/4550 family has 256 bytes of data EEPROM, addressed
 * by EEADR alone (no EEADRH on this family). All in the Access Bank. */
#define PIC_REG_EECON1        0xFA6U   /**< EEPROM/Flash control (RD/WR/WREN/WRERR/EEPGD). */
#define PIC_REG_EECON2        0xFA7U   /**< EEPROM unlock (write 0x55 then 0xAA).       */
#define PIC_REG_EEDATA        0xFA8U   /**< EEPROM data register.                      */
#define PIC_REG_EEADR         0xFA9U   /**< EEPROM address register (8-bit, 0..255).    */

/* A/D Converter, DS39632E §21.0 (10-bit, 10/13 channels). Three control
 * registers: ADCON0 (channel + GO/DONE + ADON), ADCON1 (pin config PCFG +
 * Vref config VCFG), ADCON2 (clock ADCS + acquisition ACQT + ADFM). The
 * 10-bit result is in ADRESH:ADRESL. All in the Access Bank. */
#define PIC_REG_ADCON2        0xFC0U   /**< A/D control 2 (ADCS/ACQT/ADFM).            */
#define PIC_REG_ADCON1        0xFC1U   /**< A/D control 1 (PCFG/VCFG).                 */
#define PIC_REG_ADCON0        0xFC2U   /**< A/D control 0 (CHS/GO-DONE/ADON).          */
#define PIC_REG_ADRESL        0xFC3U   /**< A/D result low byte.                       */
#define PIC_REG_ADRESH        0xFC4U   /**< A/D result high byte.                       */

/* Streaming Parallel Port (SPP), DS39632E §18.0. USB-era parallel port,
 * present only on the 40/44-pin parts (4455/4550); gated so 28-pin builds
 * don't reference registers that aren't there. SPPDATA is the data byte,
 * SPPCFG configures clocks/wait-states, SPPEPS selects the USB endpoint,
 * SPPCON enables the module. */
#if PIC18FXX5X_FAMILY_HAS_SPP
#define PIC_REG_SPPDATA        0xF62U   /**< SPP data register.                         */
#define PIC_REG_SPPCFG         0xF63U   /**< SPP configuration (CLKCFG/CSEN/CLK1EN/WS).   */
#define PIC_REG_SPPEPS         0xF64U   /**< SPP endpoint select + status (ADDR/BUSY/WR/RD). */
#define PIC_REG_SPPCON         0xF65U   /**< SPP control (SPPEN/SPPOWN).                 */
#endif

/* STATUS bits (Register 5-2). */
#define PIC_STATUS_N          EPIC_BIT(4)   /**< Negative / borrow complement. */
#define PIC_STATUS_OV         EPIC_BIT(3)   /**< Overflow.                     */
#define PIC_STATUS_Z          EPIC_BIT(2)   /**< Zero.                         */
#define PIC_STATUS_DC         EPIC_BIT(1)   /**< Digit carry/borrow.           */
#define PIC_STATUS_C          EPIC_BIT(0)   /**< Carry/borrow.                 */

/* RCON bits (Register 4-1 / 9-10). */
#define PIC_RCON_IPEN         EPIC_BIT(7)   /**< Interrupt Priority Enable.    */
#define PIC_RCON_SBOREN       EPIC_BIT(6)   /**< BOR software enable.          */
#define PIC_RCON_RI           EPIC_BIT(4)   /**< RESET instruction flag.       */
#define PIC_RCON_TO           EPIC_BIT(3)   /**< WDT time-out flag (1=not).    */
#define PIC_RCON_PD           EPIC_BIT(2)   /**< Power-down (Sleep) flag (1=not). */
#define PIC_RCON_POR          EPIC_BIT(1)   /**< Power-on Reset status.        */
#define PIC_RCON_BOR          EPIC_BIT(0)   /**< Brown-out Reset status.       */

/* INTCON bits (Register 9-1). */
#define PIC_INTCON_GIE        EPIC_BIT(7)   /**< Global Int Enable / high-priority GIE. */
#define PIC_INTCON_GIEH       PIC_INTCON_GIE
#define PIC_INTCON_PEIE       EPIC_BIT(6)   /**< Peripheral Int Enable / low-priority GIEL. */
#define PIC_INTCON_GIEL       PIC_INTCON_PEIE
#define PIC_INTCON_TMR0IE     EPIC_BIT(5)   /**< Timer0 overflow interrupt enable. */
#define PIC_INTCON_INT0IE     EPIC_BIT(4)   /**< INT0 external interrupt enable.   */
#define PIC_INTCON_RBIE       EPIC_BIT(3)   /**< RB<7:4> change interrupt enable.  */
#define PIC_INTCON_TMR0IF     EPIC_BIT(2)   /**< Timer0 overflow interrupt flag.   */
#define PIC_INTCON_INT0IF     EPIC_BIT(1)   /**< INT0 external interrupt flag.     */
#define PIC_INTCON_RBIF       EPIC_BIT(0)   /**< RB<7:4> change interrupt flag.    */

/* INTCON2 bits (Register 9-2). */
#define PIC_INTCON2_RBPU      EPIC_BIT(7)   /**< PORTB pull-up enable (active-low). */
#define PIC_INTCON2_INTEDG0   EPIC_BIT(6)   /**< INT0 edge select (1=rising).       */
#define PIC_INTCON2_INTEDG1   EPIC_BIT(5)   /**< INT1 edge select.                  */
#define PIC_INTCON2_INTEDG2   EPIC_BIT(4)   /**< INT2 edge select.                  */
#define PIC_INTCON2_TMR0IP    EPIC_BIT(2)   /**< Timer0 overflow priority (1=high). */
#define PIC_INTCON2_RBIP      EPIC_BIT(0)   /**< RB change priority (1=high).       */

/* INTCON3 bits (Register 9-3). */
#define PIC_INTCON3_INT2IP    EPIC_BIT(7)   /**< INT2 priority (1=high).            */
#define PIC_INTCON3_INT1IP    EPIC_BIT(6)   /**< INT1 priority (1=high).            */
#define PIC_INTCON3_INT2IE    EPIC_BIT(4)   /**< INT2 external interrupt enable.    */
#define PIC_INTCON3_INT1IE    EPIC_BIT(3)   /**< INT1 external interrupt enable.    */
#define PIC_INTCON3_INT2IF    EPIC_BIT(1)   /**< INT2 external interrupt flag.      */
#define PIC_INTCON3_INT1IF    EPIC_BIT(0)   /**< INT1 external interrupt flag.      */

/* PIR1 / PIE1 / IPR1 bits (Reg 9-5/9-6/9-8). */
/* Same bit layout across the three registers: flag / enable / priority. */
#define PIC_PIR1_SPPIF        EPIC_BIT(7)   /**< Streaming Parallel Port flag.  */
#define PIC_PIR1_ADIF         EPIC_BIT(6)   /**< A/D conversion complete flag.  */
#define PIC_PIR1_RCIF         EPIC_BIT(5)   /**< EUSART receive flag.           */
#define PIC_PIR1_TXIF         EPIC_BIT(4)   /**< EUSART transmit flag.          */
#define PIC_PIR1_SSPIF        EPIC_BIT(3)   /**< MSSP flag.                     */
#define PIC_PIR1_CCP1IF       EPIC_BIT(2)   /**< CCP1 flag.                     */
#define PIC_PIR1_TMR2IF       EPIC_BIT(1)   /**< Timer2 match flag.             */
#define PIC_PIR1_TMR1IF       EPIC_BIT(0)   /**< Timer1 overflow flag.          */
#define PIC_PIE1_SPPIE        EPIC_BIT(7)
#define PIC_PIE1_ADIE         EPIC_BIT(6)
#define PIC_PIE1_RCIE         EPIC_BIT(5)
#define PIC_PIE1_TXIE         EPIC_BIT(4)
#define PIC_PIE1_SSPIE        EPIC_BIT(3)
#define PIC_PIE1_CCP1IE       EPIC_BIT(2)
#define PIC_PIE1_TMR2IE       EPIC_BIT(1)
#define PIC_PIE1_TMR1IE       EPIC_BIT(0)
#define PIC_IPR1_SPPIP        EPIC_BIT(7)
#define PIC_IPR1_ADIP         EPIC_BIT(6)
#define PIC_IPR1_RCIP         EPIC_BIT(5)
#define PIC_IPR1_TXIP         EPIC_BIT(4)
#define PIC_IPR1_SSPIP        EPIC_BIT(3)
#define PIC_IPR1_CCP1IP       EPIC_BIT(2)
#define PIC_IPR1_TMR2IP       EPIC_BIT(1)
#define PIC_IPR1_TMR1IP       EPIC_BIT(0)

/* T0CON bits (Register 11-1). */
#define PIC_T0CON_TMR0ON      EPIC_BIT(7)   /**< Timer0 on/off.                  */
#define PIC_T0CON_T08BIT      EPIC_BIT(6)   /**< 1=8-bit, 0=16-bit mode.         */
#define PIC_T0CON_T0CS        EPIC_BIT(5)   /**< 0=internal Fosc/4, 1=T0CKI pin. */
#define PIC_T0CON_T0SE        EPIC_BIT(4)   /**< Counter mode edge select.       */
#define PIC_T0CON_PSA         EPIC_BIT(3)   /**< 0=prescaler assigned, 1=not.    */
#define PIC_T0CON_T0PS_MASK   0x07U         /**< T0PS2:T0PS0, prescaler ratio.   */

/* T1CON bits (Register 12-1). */
#define PIC_T1CON_RD16        EPIC_BIT(7)   /**< 16-bit read/write mode enable.   */
#define PIC_T1CON_T1RUN       EPIC_BIT(6)   /**< Timer1 system clock status (RO). */
#define PIC_T1CON_T1CKPS_MASK 0x30U         /**< T1CKPS1:T1CKPS0 at bits 5:4.     */
#define PIC_T1CON_T1OSCEN     EPIC_BIT(3)   /**< Timer1 oscillator enable.       */
#define PIC_T1CON_T1SYNC      EPIC_BIT(2)   /**< External clock sync (1=async).  */
#define PIC_T1CON_TMR1CS      EPIC_BIT(1)   /**< 0=Fosc/4, 1=external/T1OSC.     */
#define PIC_T1CON_TMR1ON      EPIC_BIT(0)   /**< Timer1 on/off.                   */

/* T2CON bits (Register 12-2). */
#define PIC_T2CON_TOUTPS_MASK 0x78U         /**< T2OUTPS3:T2OUTPS0 at bits 6:3 (1:(N+1)). */
#define PIC_T2CON_TMR2ON      EPIC_BIT(2)   /**< Timer2 on/off.                   */
#define PIC_T2CON_T2CKPS_MASK 0x03U         /**< T2CKPS1:T2CKPS0 at bits 1:0.     */

/* T3CON bits (Register 14-1). */
#define PIC_T3CON_RD16        EPIC_BIT(7)   /**< 16-bit read/write mode enable.   */
#define PIC_T3CON_T3CCP2      EPIC_BIT(6)   /**< CCP timer-select bit (with T3CCP1). */
#define PIC_T3CON_T3CKPS_MASK 0x30U         /**< T3CKPS1:T3CKPS0 at bits 5:4.     */
#define PIC_T3CON_T3CCP1      EPIC_BIT(3)   /**< CCP timer-select bit (with T3CCP2). */
#define PIC_T3CON_T3SYNC      EPIC_BIT(2)   /**< External clock sync (1=async).   */
#define PIC_T3CON_TMR3CS      EPIC_BIT(1)   /**< 0=Fosc/4, 1=external/T1OSC.      */
#define PIC_T3CON_TMR3ON      EPIC_BIT(0)   /**< Timer3 on/off.                    */

/* PIR2 / PIE2 / IPR2 bits (Reg 9-5/9-7/9-9). */
/* Same bit layout across the three registers: flag / enable / priority. */
#define PIC_PIR2_OSCFIF       EPIC_BIT(7)   /**< Oscillator fail flag.            */
#define PIC_PIR2_CMIF         EPIC_BIT(6)   /**< Comparator flag.                 */
#define PIC_PIR2_USBIF        EPIC_BIT(5)   /**< USB flag.                        */
#define PIC_PIR2_EEIF         EPIC_BIT(4)   /**< EEPROM write done flag.          */
#define PIC_PIR2_BCLIF        EPIC_BIT(3)   /**< MSSP bus collision flag.         */
#define PIC_PIR2_HLVDIF       EPIC_BIT(2)   /**< High/Low-voltage detect flag.    */
#define PIC_PIR2_TMR3IF       EPIC_BIT(1)   /**< Timer3 overflow flag.            */
#define PIC_PIR2_CCP2IF       EPIC_BIT(0)   /**< CCP2 flag.                       */
#define PIC_PIE2_OSCFIE       EPIC_BIT(7)
#define PIC_PIE2_CMIE         EPIC_BIT(6)
#define PIC_PIE2_USBIE        EPIC_BIT(5)
#define PIC_PIE2_EEIE         EPIC_BIT(4)
#define PIC_PIE2_BCLIE        EPIC_BIT(3)
#define PIC_PIE2_HLVDIE       EPIC_BIT(2)
#define PIC_PIE2_TMR3IE       EPIC_BIT(1)
#define PIC_PIE2_CCP2IE       EPIC_BIT(0)
#define PIC_IPR2_OSCFIP       EPIC_BIT(7)
#define PIC_IPR2_CMIP         EPIC_BIT(6)
#define PIC_IPR2_USBIP        EPIC_BIT(5)
#define PIC_IPR2_EEIP         EPIC_BIT(4)
#define PIC_IPR2_BCLIP        EPIC_BIT(3)
#define PIC_IPR2_HLVDIP       EPIC_BIT(2)
#define PIC_IPR2_TMR3IP       EPIC_BIT(1)
#define PIC_IPR2_CCP2IP       EPIC_BIT(0)

/* CCP1CON / CCP2CON bits (Register 16-1). */
/* CCP1 (ECCP1) has the P1M output-mode bits in <7:6>; CCP2 (plain) leaves
 * them unimplemented. DC1B/DC2B are the PWM duty LSBs (or capture/compare
 * 2 LSBs in 16-bit mode). CCP1M/CCP2M are the mode select (capture/
 * compare/PWM). DS39632E Register 16-1. */
#define PIC_CCP1_P1M_MASK     0xC0U    /**< P1M1:P1M0 at bits 7:6 (output mode). */
#define PIC_CCP1_DC1B_MASK    0x30U    /**< DC1B1:DC1B0 at bits 5:4 (duty LSBs). */
#define PIC_CCP1_M_MASK       0x0FU    /**< CCP1M3:CCP1M0 at bits 3:0 (mode).    */
#define PIC_CCP2_DC2B_MASK    0x30U    /**< DC2B1:DC2B0 at bits 5:4.            */
#define PIC_CCP2_M_MASK       0x0FU    /**< CCP2M3:CCP2M0 at bits 3:0.          */

/* ECCP1DEL bits (Register 16-2). */
#define PIC_ECCP1DEL_PRSEN    EPIC_BIT(7)  /**< PWM restart enable (auto-restart). */
#define PIC_ECCP1DEL_PDC_MASK 0x7FU        /**< PDC6:PDC0, dead-band delay (6:0). */

/* ECCP1AS bits (Register 16-3). */
#define PIC_ECCP1AS_ECCPASE   EPIC_BIT(7)  /**< Auto-shutdown event status (RO once active). */
#define PIC_ECCP1AS_SRC_MASK  0x70U        /**< ECCPAS2:ECCPAS0 at bits 6:4 (source). */
#define PIC_ECCP1AS_PSSAC_MASK 0x0CU       /**< PSSAC1:PSSAC0 at bits 3:2 (P1A/P1C state). */
#define PIC_ECCP1AS_PSSBD_MASK 0x03U       /**< PSSBD1:PSSBD0 at bits 1:0 (P1B/P1D state). */

/* MSSP bits (Register 19-1/19-2/19-4/19-5). */
/* SSPCON1 (mode/enable/polarity/flags), SSPSTAT (status), SSPCON2 (I2C
 * master control). Same bit positions as the PIC16 SSP. DS39632E §19.0. */
#define PIC_SSPCON1_SSPM_MASK  0x0FU       /**< SSPM3:SSPM0 at bits 3:0 (mode select). */
#define PIC_SSPCON1_CKP        EPIC_BIT(4)  /**< Clock polarity (SPI).                */
#define PIC_SSPCON1_SSPEN      EPIC_BIT(5)  /**< MSSP enable.                          */
#define PIC_SSPCON1_SSPOV      EPIC_BIT(6)  /**< Receive overflow.                    */
#define PIC_SSPCON1_WCOL       EPIC_BIT(7)  /**< Write collision.                     */
#define PIC_SSPCON2_SEN        EPIC_BIT(0)  /**< Start condition enable (I2C master). */
#define PIC_SSPCON2_RSEN       EPIC_BIT(1)  /**< Repeated start enable.               */
#define PIC_SSPCON2_PEN        EPIC_BIT(2)  /**< Stop condition enable.               */
#define PIC_SSPCON2_RCEN       EPIC_BIT(3)  /**< Receive enable.                      */
#define PIC_SSPCON2_ACKEN      EPIC_BIT(4)  /**< Acknowledge sequence enable.         */
#define PIC_SSPCON2_ACKDT      EPIC_BIT(5)  /**< Acknowledge data (ACK/NACK value).    */
#define PIC_SSPCON2_ACKSTAT    EPIC_BIT(6)  /**< Acknowledge status (from slave).      */
#define PIC_SSPCON2_GCEN       EPIC_BIT(7)  /**< General call enable (I2C slave).      */
#define PIC_SSPSTAT_BF         EPIC_BIT(0)  /**< Buffer full.                          */
#define PIC_SSPSTAT_UA         EPIC_BIT(1)  /**< Update address (10-bit).              */
#define PIC_SSPSTAT_RW         EPIC_BIT(2)  /**< Read/write (I2C).                     */
#define PIC_SSPSTAT_S          EPIC_BIT(3)  /**< Start (I2C).                          */
#define PIC_SSPSTAT_P          EPIC_BIT(4)  /**< Stop (I2C).                           */
#define PIC_SSPSTAT_DA         EPIC_BIT(5)  /**< Data/address (I2C).                   */
#define PIC_SSPSTAT_CKE        EPIC_BIT(6)  /**< Clock edge (SPI).                     */
#define PIC_SSPSTAT_SMP        EPIC_BIT(7)  /**< Sample phase (SPI master).             */

/* EUSART bits (Register 20-1/20-2/20-3). */
/* TXSTA (Register 20-1) and RCSTA (Register 20-2) have the same bit layout
 * as the PIC16 USART, so the shared PIC_TXSTA_* / PIC_RCSTA_* names resolve
 * identically across families (only the PIC_REG_* address differs).
 * BAUDCON (Register 20-3) is the PIC18 EUSART addition. DS39632E §20.0. */
#define PIC_TXSTA_TX9D         EPIC_BIT(0)  /**< 9th bit of TX data.                  */
#define PIC_TXSTA_TRMT         EPIC_BIT(1)  /**< TSR empty (read-only).               */
#define PIC_TXSTA_BRGH         EPIC_BIT(2)  /**< High baud rate.                      */
#define PIC_TXSTA_SYNC         EPIC_BIT(4)  /**< Sync mode.                           */
#define PIC_TXSTA_TXEN         EPIC_BIT(5)  /**< TX enable.                           */
#define PIC_TXSTA_TX9          EPIC_BIT(6)  /**< 9-bit TX.                            */
#define PIC_TXSTA_CSRC         EPIC_BIT(7)  /**< Clock source (sync).                 */

#define PIC_RCSTA_RX9D         EPIC_BIT(0)  /**< 9th bit of RX data.                  */
#define PIC_RCSTA_OERR         EPIC_BIT(1)  /**< Overrun error.                       */
#define PIC_RCSTA_FERR         EPIC_BIT(2)  /**< Framing error.                       */
#define PIC_RCSTA_ADDEN        EPIC_BIT(3)  /**< Address detect (9-bit).              */
#define PIC_RCSTA_CREN         EPIC_BIT(4)  /**< Continuous receive.                  */
#define PIC_RCSTA_SREN         EPIC_BIT(5)  /**< Single receive.                      */
#define PIC_RCSTA_RX9          EPIC_BIT(6)  /**< 9-bit RX.                            */
#define PIC_RCSTA_SPEN         EPIC_BIT(7)  /**< Serial port enable.                  */

/* BAUDCON (Register 20-3) — the EUSART-specific control/status register. */
#define PIC_BAUDCON_ABDEN      EPIC_BIT(0)  /**< Auto-baud detect enable.            */
#define PIC_BAUDCON_WUE        EPIC_BIT(1)  /**< Wake-up enable (async).             */
#define PIC_BAUDCON_BRG16      EPIC_BIT(3)  /**< 16-bit baud generator (else 8-bit). */
#define PIC_BAUDCON_TXCKP      EPIC_BIT(4)  /**< Sync: TX clock polarity.            */
#define PIC_BAUDCON_RXDTP      EPIC_BIT(5)  /**< Sync: RX data polarity.             */
#define PIC_BAUDCON_RCIDL      EPIC_BIT(6)  /**< Receiver idle (read-only).          */
#define PIC_BAUDCON_ABDOVF     EPIC_BIT(7)  /**< Auto-baud overflow (read/clear).    */

/* CMCON bits (Register 22-1). */
/* Same bit layout as the PIC16 CMCON (DS39632E §22.0). Eight operating
 * modes are selected by CM2:CM0; POR default is 111 (comparators off). */
#define PIC_CMCON_CM_MASK      0x07U        /**< CM2:CM0 at bits 2:0 (mode select). */
#define PIC_CMCON_CIS          EPIC_BIT(3)  /**< Comparator input switch.            */
#define PIC_CMCON_C1INV        EPIC_BIT(4)  /**< C1 output inversion.                 */
#define PIC_CMCON_C2INV        EPIC_BIT(5)  /**< C2 output inversion.                 */
#define PIC_CMCON_C1OUT        EPIC_BIT(6)  /**< C1 output (read-only).              */
#define PIC_CMCON_C2OUT        EPIC_BIT(7)  /**< C2 output (read-only).              */

/* EECON1 bits (Register 7-1). */
/* RD/WR/WREN/WRERR mirror the PIC16 EECON1; EEPGD/CFGS/FREE select data
 * EEPROM vs program flash vs config (DS39632E §7.0). For data EEPROM
 * access, EEPGD = 0 and CFGS = 0. */
#define PIC_EECON1_RD          EPIC_BIT(0)  /**< Read control (strobe, self-clears). */
#define PIC_EECON1_WR          EPIC_BIT(1)  /**< Write control (strobe).             */
#define PIC_EECON1_WREN        EPIC_BIT(2)  /**< Write enable.                       */
#define PIC_EECON1_WRERR       EPIC_BIT(3)  /**< Write-error flag.                   */
#define PIC_EECON1_FREE        EPIC_BIT(4)  /**< Flash row erase enable.             */
#define PIC_EECON1_CFGS        EPIC_BIT(6)  /**< Config access (1) vs code/data (0). */
#define PIC_EECON1_EEPGD       EPIC_BIT(7)  /**< 0 = data EEPROM, 1 = program flash. */

/* ADCON0 bits (Register 21-1). */
/* ADON bit0, GO/DONE bit1, CHS3:CHS0 bits 5:2 (4-bit channel). */
#define PIC_ADCON0_ADON        EPIC_BIT(0)  /**< A/D module on.                       */
#define PIC_ADCON0_GO_DONE     EPIC_BIT(1)  /**< Conversion status (1 = in progress). */
#define PIC_ADCON0_CHS_MASK    0x3CU        /**< CHS3:CHS0 at bits 5:2.               */
#define PIC_ADCON0_CHS_POS     2            /**< CHS field shift.                     */

/* ADCON1 bits (Register 21-2). */
/* PCFG3:PCFG0 bits 3:0 (port config, Table 21-3); VCFG0=Vref+ (bit4),
 * VCFG1=Vref- (bit5). */
#define PIC_ADCON1_PCFG_MASK   0x0FU        /**< PCFG3:PCFG0 at bits 3:0.             */
#define PIC_ADCON1_VCFG0       EPIC_BIT(4)  /**< Vref+ source: 1=AN3, 0=VDD.          */
#define PIC_ADCON1_VCFG1       EPIC_BIT(5)  /**< Vref- source: 1=AN2, 0=VSS.           */

/* ADCON2 bits (Register 21-3). */
/* ADCS2:ADCS0 bits 2:0 (clock), ACQT2:ACQT0 bits 5:3 (acquisition), ADFM bit7. */
#define PIC_ADCON2_ADCS_MASK   0x07U        /**< ADCS2:ADCS0 at bits 2:0.             */
#define PIC_ADCON2_ACQT_MASK   0x38U        /**< ACQT2:ACQT0 at bits 5:3.             */
#define PIC_ADCON2_ACQT_POS    3            /**< ACQT field shift.                     */
#define PIC_ADCON2_ADFM        EPIC_BIT(7)  /**< 1 = right justified, 0 = left.       */

/* SPP bits (Register 18-1/18-2/18-3). */
/* Gated to 40/44-pin parts (PIC18FXX5X_FAMILY_HAS_SPP). DS39632E §18.0. */
#if PIC18FXX5X_FAMILY_HAS_SPP
/* SPPCON (Register 18-1). */
#define PIC_SPPCON_SPPEN       EPIC_BIT(0)  /**< SPP enable.                       */
#define PIC_SPPCON_SPPOWN      EPIC_BIT(1)  /**< Ownership: 1=USB, 0=MCU.          */
/* SPPCFG (Register 18-2). */
#define PIC_SPPCFG_WS_MASK     0x0FU        /**< WS3:WS0 at bits 3:0 (0..30 wait states, x2). */
#define PIC_SPPCFG_CLK1EN      EPIC_BIT(4)  /**< RE0 as SPP CLK1 output.           */
#define PIC_SPPCFG_CSEN        EPIC_BIT(5)  /**< RB4 as SPP CS output.             */
#define PIC_SPPCFG_CLKCFG_MASK 0xC0U        /**< CLKCFG1:CLKCFG0 at bits 7:6.      */
#define PIC_SPPCFG_CLKCFG_POS  6            /**< CLKCFG field shift.                */
/* SPPEPS (Register 18-3). ADDR3:ADDR0 at bits 3:0, status bits read-only. */
#define PIC_SPPEPS_ADDR_MASK   0x0FU        /**< Endpoint address at bits 3:0.     */
#define PIC_SPPEPS_SPPBUSY     EPIC_BIT(4)  /**< SPP busy (read-only).             */
#define PIC_SPPEPS_WRSPP       EPIC_BIT(6)  /**< Write occurred (read-only status). */
#define PIC_SPPEPS_RDSPP       EPIC_BIT(7)  /**< Read occurred (read-only status).  */
#endif

/* Reset values (POR). */
/* DS39632E Table 5-1 "Value at POR" column + Register 4-1 reset notes.
 * RCON after POR: IPEN=0, SBOREN=1, RI=0, TO=1, PD=1, POR=1, BOR=1
 * (POR/BOR set per Register 4-1 Note 1). IPR1 defaults to all-high
 * priority. T0CON resets to 0xFF (on, 8-bit, external, no prescaler). */
#define PIC_STATUS_POR_VALUE     0x00U
#define PIC_BSR_POR_VALUE        0x00U
#define PIC_RCON_POR_VALUE       0x57U
#define PIC_INTCON_POR_VALUE     0x00U
#define PIC_INTCON2_POR_VALUE    0xFBU
#define PIC_INTCON3_POR_VALUE    0xC0U
#define PIC_PIR1_POR_VALUE       0x00U
#define PIC_PIE1_POR_VALUE       0x00U
#define PIC_IPR1_POR_VALUE       0xFFU
#define PIC_T0CON_POR_VALUE      0xFFU
#define PIC_T1CON_POR_VALUE      0x00U
#define PIC_T2CON_POR_VALUE      0x00U
#define PIC_T3CON_POR_VALUE      0x00U
#define PIC_PR2_POR_VALUE        0xFFU
#define PIC_PIR2_POR_VALUE       0x00U
#define PIC_PIE2_POR_VALUE       0x00U
#define PIC_IPR2_POR_VALUE       0xFFU
#define PIC_CCP1CON_POR_VALUE    0x00U
#define PIC_CCP2CON_POR_VALUE    0x00U
#define PIC_ECCP1DEL_POR_VALUE   0x00U
#define PIC_ECCP1AS_POR_VALUE    0x00U
#define PIC_SSPCON1_POR_VALUE    0x00U
#define PIC_SSPCON2_POR_VALUE    0x00U
#define PIC_SSPSTAT_POR_VALUE    0x00U
#define PIC_SSPADD_POR_VALUE     0x00U
/* EUSART: TXSTA resets to 0x02 (TRMT=1, TSR empty); the rest are clear.
 * DS39632E Table 5-1. */
#define PIC_BAUDCON_POR_VALUE   0x00U
#define PIC_RCSTA_POR_VALUE     0x00U
#define PIC_TXSTA_POR_VALUE     0x02U
#define PIC_SPBRG_POR_VALUE     0x00U
#define PIC_SPBRGH_POR_VALUE    0x00U
/* CMCON resets to 0x07 (CM2:CM0 = 111, comparators off — the POR default,
 * DS39632E Figure 22-1). */
#define PIC_CMCON_POR_VALUE     0x07U
/* EECON1 resets to 0x00 (RD/WR/WREN clear, EEPGD=0 -> data EEPROM).
 * DS39632E Table 5-1. */
#define PIC_EECON1_POR_VALUE    0x00U
/* ADC: ADCON0/ADCON1/ADCON2 reset to 0x00 (DS39632E Table 5-1). PCFG
 * POR technically depends on the PBADEN config bit; the sim uses 0x00. */
#define PIC_ADCON0_POR_VALUE    0x00U
#define PIC_ADCON1_POR_VALUE    0x00U
#define PIC_ADCON2_POR_VALUE    0x00U
/* SPP registers reset to 0x00 (DS39632E Table 5-1); 40/44-pin only. */
#if PIC18FXX5X_FAMILY_HAS_SPP
#define PIC_SPPCON_POR_VALUE     0x00U
#define PIC_SPPCFG_POR_VALUE     0x00U
#define PIC_SPPEPS_POR_VALUE     0x00U
#endif
#define PIC_TRIS_POR_VALUE       0xFFU   /* All pins inputs after POR. */
#define PIC_LAT_POR_VALUE        0x00U   /* Output latches clear after POR. */
#define PIC_PORT_POR_VALUE       0x00U

#endif /* PIC18FXX5X_SFR_H */
