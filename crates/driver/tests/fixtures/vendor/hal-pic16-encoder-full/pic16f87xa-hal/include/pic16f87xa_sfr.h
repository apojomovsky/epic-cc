/* SFR address map for the PIC16F87XA family. Every address, bit mask
 * and reset value is 1-to-1 from DS39582B (Figures 2-3/2-4, Tables 3-1,
 * 4-2..4-10, 14-1); family-dependent registers (PORTD/E on 28-pin
 * parts) are guarded by PIC16F87XA_FAMILY_HAS_*. */

#ifndef PIC16F87XA_SFR_H
#define PIC16F87XA_SFR_H

#include "pic16f87xa.h"

/* Bank 0, core SFRs. */

/** Indirect address pointer.                    DS39582B §2.2, addr 00h. */
#define PIC_REG_INDF          0x00U
/** Option register (Bank 1).                     DS39582B §2.2, addr 81h. */
#define PIC_REG_OPTION        0x81U
/** Program Counter low byte.                    DS39582B §2.2, addr 02h. */
#define PIC_REG_PCL           0x02U
/** Status register.                             DS39582B §2.2, addr 03h. */
#define PIC_REG_STATUS        0x03U
/** File Select Register (indirect addressing).  DS39582B §2.2, addr 04h. */
#define PIC_REG_FSR           0x04U

/* I/O ports, Tables 4-2..4-10. */
#define PIC_REG_PORTA         0x05U
#define PIC_REG_PORTB         0x06U
#define PIC_REG_PORTC         0x07U
#define PIC_REG_PORTD         0x08U   /* 40/44-pin only. */
#define PIC_REG_PORTE         0x09U   /* 40/44-pin only. */

#define PIC_REG_TRISA         0x85U
#define PIC_REG_TRISB         0x86U
#define PIC_REG_TRISC         0x87U
#define PIC_REG_TRISD         0x88U   /* 40/44-pin only. */
#define PIC_REG_TRISE         0x89U   /* 40/44-pin only. */

/* Core CPU control, DS39582B §14.10, §14.11, Table 14-4. */
#define PIC_REG_PCLATH        0x0AU
#define PIC_REG_INTCON        0x0BU
#define PIC_REG_PIR1          0x0CU
#define PIC_REG_PIR2          0x0DU
#define PIC_REG_PIE1          0x8CU   /* Bank 1, DS39582B Table 14-4. */
#define PIC_REG_PIE2          0x8DU   /* Bank 1. */
#define PIC_REG_PCON          0x8EU

/* Timer0, DS39582B §5.0. */
#define PIC_REG_TMR0          0x01U
#define PIC_REG_TMR0L         0x0EU   /* (unused on F87XA, kept for naming). */

/* Timer1, DS39582B §6.0. */
#define PIC_REG_TMR1L         0x0EU
#define PIC_REG_TMR1H         0x0FU
#define PIC_REG_T1CON         0x10U

/* Timer2, DS39582B §7.0. */
#define PIC_REG_TMR2          0x11U
#define PIC_REG_T2CON         0x12U

/* SSP, DS39582B §9.0. */
#define PIC_REG_SSPBUF        0x13U
#define PIC_REG_SSPCON        0x14U

/* CCP, DS39582B §8.0. */
#define PIC_REG_CCP1RL        0x15U
#define PIC_REG_CCP1RH        0x16U
#define PIC_REG_CCP1CON       0x17U

/* USART, DS39582B §10.0. */
#define PIC_REG_RCSTA         0x18U
#define PIC_REG_TXREG         0x19U
#define PIC_REG_RCREG         0x1AU
#define PIC_REG_CCPR2L        0x1BU
#define PIC_REG_CCPR2H        0x1CU
#define PIC_REG_CCP2CON       0x1DU

/* ADC, DS39582B §11.0. */
#define PIC_REG_ADRESH        0x1EU
#define PIC_REG_ADCON0        0x1FU

/* Bank 1. */

/* SSP, DS39582B §9.0, Bank 1. */
#define PIC_REG_SSPCON2       0x91U
#define PIC_REG_PR2           0x92U
#define PIC_REG_SSPADD        0x93U
#define PIC_REG_SSPSTAT       0x94U

/* USART, DS39582B §10.0, Bank 1. */
#define PIC_REG_TXSTA         0x98U
#define PIC_REG_SPBRG         0x99U

/* Comparators + Vref, DS39582B §12.0, §13.0, Register 12-1 / 13-1. */
#define PIC_REG_CMCON         0x9CU
#define PIC_REG_CVRCON        0x9DU

/* ADC, DS39582B §11.0, Bank 1. */
#define PIC_REG_ADRESL        0x9EU
#define PIC_REG_ADCON1        0x9FU

/* Bank 2, EEPROM. */

/* DS39582B §3.0, Table 3-1. */
#define PIC_REG_EEDATA        0x10CU
#define PIC_REG_EEADR         0x10DU
#define PIC_REG_EEDATH        0x10EU
#define PIC_REG_EEADRH        0x10FU
#define PIC_REG_EECON1        0x18CU
#define PIC_REG_EECON2        0x18DU

/* STATUS register bits. */

/** Carry / Borrow.                              DS39582B §2.2 / Register 2-1. */
#define PIC_STATUS_C          EPIC_BIT(0)
/** Digit Carry.                                  DS39582B Register 2-1. */
#define PIC_STATUS_DC         EPIC_BIT(1)
/** Zero.                                         DS39582B Register 2-1. */
#define PIC_STATUS_Z          EPIC_BIT(2)
/** Power-down.                                   DS39582B §14.14, Register 2-1. */
#define PIC_STATUS_PD         EPIC_BIT(3)
/** Time-out.                                     DS39582B §14.13, Register 2-1. */
#define PIC_STATUS_TO         EPIC_BIT(4)
/** Register Page Select bits (RP0:RP1).          DS39582B Register 2-1. */
#define PIC_STATUS_RP0        EPIC_BIT(5)
#define PIC_STATUS_RP1        EPIC_BIT(6)
/** IRP, unused on PIC16F87XA, reads as 0.       DS39582B Register 2-1. */
#define PIC_STATUS_IRP        EPIC_BIT(7)

/* INTCON register bits. */

/** RB0/INT external interrupt flag.             DS39582B §14.11.1, Reg 14-3. */
#define PIC_INTCON_INTF       EPIC_BIT(1)
/** RB port change interrupt flag.               DS39582B §14.11.3, Reg 14-3. */
#define PIC_INTCON_RBIF       EPIC_BIT(0)
/** TMR0 overflow interrupt flag.                DS39582B §14.11.2, Reg 14-3. */
#define PIC_INTCON_TMR0IF     EPIC_BIT(2)
/** RB0/INT external interrupt enable.           DS39582B §14.11.1, Reg 14-3. */
#define PIC_INTCON_INTE       EPIC_BIT(4)
/** RB port change interrupt enable.             DS39582B §14.11.3, Reg 14-3. */
#define PIC_INTCON_RBIE       EPIC_BIT(3)
/** TMR0 overflow interrupt enable.              DS39582B §14.11.2, Reg 14-3. */
#define PIC_INTCON_TMR0IE     EPIC_BIT(5)
/** Peripheral interrupt enable.                 DS39582B §14.11, Reg 14-3. */
#define PIC_INTCON_PEIE       EPIC_BIT(6)
/** Global interrupt enable.                     DS39582B §14.11, Reg 14-3. */
#define PIC_INTCON_GIE        EPIC_BIT(7)

/* PIR1 / PIE1. */

/* DS39582B §14.11, Register 14-3. */
#define PIC_PIR1_TMR1IF       EPIC_BIT(0)
#define PIC_PIR1_TMR2IF       EPIC_BIT(1)
#define PIC_PIR1_CCP1IF       EPIC_BIT(2)
#define PIC_PIR1_SSPIF        EPIC_BIT(3)
#define PIC_PIR1_TXIF         EPIC_BIT(4)
#define PIC_PIR1_RCIF         EPIC_BIT(5)
#define PIC_PIR1_ADIF         EPIC_BIT(6)
#define PIC_PIR1_PSPIF        EPIC_BIT(7)   /* 40/44-pin only. */

#define PIC_PIE1_TMR1IE       EPIC_BIT(0)
#define PIC_PIE1_TMR2IE       EPIC_BIT(1)
#define PIC_PIE1_CCP1IE       EPIC_BIT(2)
#define PIC_PIE1_SSPIE        EPIC_BIT(3)
#define PIC_PIE1_TXIE         EPIC_BIT(4)
#define PIC_PIE1_RCIE         EPIC_BIT(5)
#define PIC_PIE1_ADIE         EPIC_BIT(6)
#define PIC_PIE1_PSPIE        EPIC_BIT(7)

/* PIR2 / PIE2. */

/* DS39582B §14.11, Register 14-4. */
#define PIC_PIR2_CCP2IF       EPIC_BIT(0)
#define PIC_PIR2_BCLIF        EPIC_BIT(3)
#define PIC_PIR2_EEIF         EPIC_BIT(4)
#define PIC_PIR2_CMIF         EPIC_BIT(6)
/* No OSFIF on the 87XA: PIR2 bit 7 is unimplemented. */
#define PIC_PIE2_CCP2IE       EPIC_BIT(0)
#define PIC_PIE2_BCLIE        EPIC_BIT(3)
#define PIC_PIE2_EEIE         EPIC_BIT(4)
#define PIC_PIE2_CMIE         EPIC_BIT(6)

/* Reset values (POR). */

/* DS39582B §14, Table 14-6. */
#define PIC_STATUS_POR_VALUE     0x18U  /* 0001 1xxx, IRP=0,RP1=0,RP0=0,TO=1,PD=1,... */
#define PIC_PCON_POR_VALUE       0x0FU  /* BOR and POR flags unknown. */
#define PIC_INTCON_POR_VALUE     0x00U
#define PIC_PIR1_POR_VALUE       0x00U
#define PIC_PIR2_POR_VALUE       0x00U
#define PIC_PIE1_POR_VALUE       0x00U
#define PIC_PIE2_POR_VALUE       0x00U
#define PIC_T1CON_POR_VALUE      0x00U
#define PIC_T2CON_POR_VALUE      0x00U
#define PIC_ADCON0_POR_VALUE     0x00U
#define PIC_ADCON1_POR_VALUE     0x00U

/* OPTION_REG bits (Timer0 + WDT). */

/* DS39582B §5.3, Register 5-1. */
#define PIC_OPTION_RBPU         EPIC_BIT(7)   /* PORTB pull-up enable (active-low). */
#define PIC_OPTION_INTEDG       EPIC_BIT(6)   /* INT edge select.                */
#define PIC_OPTION_T0CS         EPIC_BIT(5)   /* TMR0 clock source.              */
#define PIC_OPTION_T0SE         EPIC_BIT(4)   /* TMR0 source edge.               */
#define PIC_OPTION_PSA          EPIC_BIT(3)   /* Prescaler assignment.           */
#define PIC_OPTION_PS_MASK      0x07U                /* PS2:PS0, prescaler ratio.     */

/* T1CON bits (Timer1). */

/* DS39582B §6.0, Register 6-1. */
#define PIC_T1CON_TMR1ON        EPIC_BIT(0)
#define PIC_T1CON_TMR1CS        EPIC_BIT(1)
#define PIC_T1CON_T1SYNC        EPIC_BIT(2)
#define PIC_T1CON_T1OSCEN       EPIC_BIT(3)
#define PIC_T1CON_T1CKPS0       EPIC_BIT(4)
#define PIC_T1CON_T1CKPS1       EPIC_BIT(5)

/* T2CON bits (Timer2). */

/* DS39582B §7.0, Register 7-1. */
#define PIC_T2CON_T2CKPS_MASK   0x03U                /* T2CKPS1:T2CKPS0, bits 0..1. */
#define PIC_T2CON_TMR2ON        EPIC_BIT(2)
#define PIC_T2CON_TOUTPS_MASK   0x78U                /* TOUTPS3:TOUTPS0, bits 3..6. */
#define PIC_T2CON_TOUTPS_POS    3U

/* CCPxCON bits. */

/* DS39582B §8.0, Register 8-1.
 * The two LSBs of the 10-bit PWM duty cycle live in CCPxCON<5:4>
 * (CCPxY:CCPxX), see §8.3.2. */
#define PIC_CCP_CCPX_M0         EPIC_BIT(0)    /* CCPxM0, mode bit 0.        */
#define PIC_CCP_CCPX_M1         EPIC_BIT(1)    /* CCPxM1, mode bit 1.        */
#define PIC_CCP_CCPX_M2         EPIC_BIT(2)    /* CCPxM2, mode bit 2.        */
#define PIC_CCP_CCPX_M3         EPIC_BIT(3)    /* CCPxM3, mode bit 3.        */
#define PIC_CCP_CCPX_Y          EPIC_BIT(4)    /* PWM duty LSB bit 0.         */
#define PIC_CCP_CCPX_X          EPIC_BIT(5)    /* PWM duty LSB bit 1.         */

/* TXSTA bits (USART). */

/* DS39582B §10.0, Register 10-1. */
#define PIC_TXSTA_TX9D         EPIC_BIT(0)    /* 9th bit of TX data. */
#define PIC_TXSTA_TRMT         EPIC_BIT(1)    /* TSR empty. */
#define PIC_TXSTA_BRGH         EPIC_BIT(2)    /* High baud rate. */
#define PIC_TXSTA_SYNC         EPIC_BIT(4)    /* Sync mode. */
#define PIC_TXSTA_TXEN         EPIC_BIT(5)    /* TX enable. */
#define PIC_TXSTA_TX9          EPIC_BIT(6)    /* 9-bit TX. */
#define PIC_TXSTA_CSRC         EPIC_BIT(7)    /* Clock source (sync). */

/* RCSTA bits (USART). */

/* DS39582B §10.0, Register 10-2. */
#define PIC_RCSTA_RX9D         EPIC_BIT(0)    /* 9th bit of RX data. */
#define PIC_RCSTA_OERR         EPIC_BIT(1)    /* Overrun error. */
#define PIC_RCSTA_FERR         EPIC_BIT(2)    /* Framing error. */
#define PIC_RCSTA_ADDEN        EPIC_BIT(3)    /* Address detect (9-bit). */
#define PIC_RCSTA_CREN         EPIC_BIT(4)    /* Continuous receive. */
#define PIC_RCSTA_SREN         EPIC_BIT(5)    /* Single receive. */
#define PIC_RCSTA_RX9          EPIC_BIT(6)    /* 9-bit RX. */
#define PIC_RCSTA_SPEN         EPIC_BIT(7)    /* Serial port enable. */

/* SSPCON / SSPSTAT bits (MSSP). */

/* DS39582B §9.0, Registers 9-1 (SSPSTAT), 9-2/9-4 (SSPCON),
 * 9-5 (SSPCON2). The same SSPCON<3:0> field selects mode in both
 * SPI and I²C operation. */
#define PIC_SSPCON_SSPM_MASK   0x0FU                /* SSPM3:SSPM0.       */
#define PIC_SSPCON_CKP         EPIC_BIT(4)    /* Clock polarity.    */
#define PIC_SSPCON_SSPEN       EPIC_BIT(5)    /* SSP enable.        */
#define PIC_SSPCON_SSPOV       EPIC_BIT(6)    /* Receive overflow.  */
#define PIC_SSPCON_WCOL        EPIC_BIT(7)    /* Write collision.   */

/* SSPCON2 (I²C only), Register 9-5. */
#define PIC_SSPCON2_SEN        EPIC_BIT(0)    /* Start condition enable. */
#define PIC_SSPCON2_RSEN       EPIC_BIT(1)    /* Repeated start enable.  */
#define PIC_SSPCON2_PEN        EPIC_BIT(2)    /* Stop condition enable.  */
#define PIC_SSPCON2_RCEN       EPIC_BIT(3)    /* Receive enable.         */
#define PIC_SSPCON2_ACKEN      EPIC_BIT(4)    /* Acknowledge sequence.   */
#define PIC_SSPCON2_ACKDT      EPIC_BIT(5)    /* Acknowledge data.       */
#define PIC_SSPCON2_ACKSTAT    EPIC_BIT(6)    /* Acknowledge status.     */
#define PIC_SSPCON2_GCEN       EPIC_BIT(7)    /* General call enable.    */

/* SSPSTAT, Register 9-1. */
#define PIC_SSPSTAT_BF         EPIC_BIT(0)    /* Buffer full.        */
#define PIC_SSPSTAT_UA         EPIC_BIT(1)    /* Update address.     */
#define PIC_SSPSTAT_RW         EPIC_BIT(2)    /* Read/write (I²C).    */
#define PIC_SSPSTAT_S          EPIC_BIT(3)    /* Start (I²C).         */
#define PIC_SSPSTAT_P          EPIC_BIT(4)    /* Stop (I²C).          */
#define PIC_SSPSTAT_DA         EPIC_BIT(5)    /* Data/address (I²C).  */
#define PIC_SSPSTAT_CKE        EPIC_BIT(6)    /* Clock edge (SPI).    */
#define PIC_SSPSTAT_SMP        EPIC_BIT(7)    /* Sample bit (SPI).    */

/* ADCON0 / ADCON1 bits (A/D). */

/* DS39582B §11.0, Registers 11-1 (ADCON0) and 11-2 (ADCON1). */
#define PIC_ADCON0_ADON        EPIC_BIT(0)    /* A/D on.        */
#define PIC_ADCON0_GO_DONE     EPIC_BIT(2)    /* Start / status. */
#define PIC_ADCON0_CHS_MASK    0x1CU                /* CHS2:CHS0, bits 5:3. */
#define PIC_ADCON0_CHS_POS     3U
#define PIC_ADCON0_ADCS_MASK   0xC0U                /* ADCS1:ADCS0, bits 7:6. */
#define PIC_ADCON0_ADCS_POS    6U

#define PIC_ADCON1_PCFG_MASK   0x0FU                /* PCFG3:PCFG0, bits 3:0. */
#define PIC_ADCON1_ADCS2       EPIC_BIT(6)    /* ADCS2, bit 6. */
#define PIC_ADCON1_ADFM        EPIC_BIT(7)    /* Result format. */

/* CMCON bits (Comparator). */

/* DS39582B §12.0, Register 12-1. */
#define PIC_CMCON_CM_MASK      0x07U                /* CM2:CM0, bits 2:0. */
#define PIC_CMCON_CIS          EPIC_BIT(3)    /* Comparator input switch. */
#define PIC_CMCON_C1INV        EPIC_BIT(4)    /* C1 output invert. */
#define PIC_CMCON_C2INV        EPIC_BIT(5)    /* C2 output invert. */
#define PIC_CMCON_C1OUT        EPIC_BIT(6)    /* C1 output (read-only). */
#define PIC_CMCON_C2OUT        EPIC_BIT(7)    /* C2 output (read-only). */

/* CVRCON bits (Vref). */

/* DS39582B §13.0, Register 13-1. */
#define PIC_CVRCON_CVR_MASK    0x0FU                /* CVR3:CVR0, bits 3:0. */
#define PIC_CVRCON_CVRR         EPIC_BIT(5)    /* Vref range select. */
#define PIC_CVRCON_CVROE        EPIC_BIT(6)    /* Vref output enable. */
#define PIC_CVRCON_CVREN        EPIC_BIT(7)    /* Vref circuit enable. */

/* EECON1 bits (EEPROM). */

/* DS39582B §3.0, Register 3-1. */
#define PIC_EECON1_RD          EPIC_BIT(0)    /* Read control.   */
#define PIC_EECON1_WR          EPIC_BIT(1)    /* Write control.  */
#define PIC_EECON1_WREN        EPIC_BIT(2)    /* Write enable.   */
#define PIC_EECON1_WRERR       EPIC_BIT(3)    /* Write error.    */
/* No EEIF in EECON1: the EEPROM write-done flag is PIR2<EEIF>
 * (PIC_PIR2_EEIF). */

/* TRISE bits (PORT E and PSP). */

/* DS39582B §4.5, Register 4-9. PSP is 40/44-pin only. The status bits
 * are the UPPER half (IBF bit 7, OBF bit 6, IBOV bit 5, PSPMODE bit 4);
 * bits 2:0 are the RE2:RE0 direction controls and bit 3 is
 * unimplemented. The PSP interrupt enable is PIE1<7> (PIC_PIE1_PSPIE),
 * not a TRISE bit. */
#define PIC_TRISE_IBF          EPIC_BIT(7)    /* Input buffer full.        */
#define PIC_TRISE_OBF          EPIC_BIT(6)    /* Output buffer full.       */
#define PIC_TRISE_IBOV         EPIC_BIT(5)    /* Input buffer overflow.    */
#define PIC_TRISE_PSPMODE      EPIC_BIT(4)    /* PSP mode select.          */

/* Bank-selection helper. */

/* Set the bank-select bits RP1:RP0 in STATUS to access a given bank
 * (DS39582B §2.2, Table 2-1). A macro, not a static inline: a call
 * boundary corrupts Bank 1 writes under XC8 v4.00 (see README.md, XC8
 * codegen gotchas), and combined calls can hang XC8's cgpic pass. */
#define pic_select_bank(bank)                                          \
    do {                                                               \
        uint8_t pic_select_bank_status_ = EPIC_REG8(PIC_REG_STATUS);   \
        pic_select_bank_status_ &=                                     \
            (uint8_t)~(PIC_STATUS_RP0 | PIC_STATUS_RP1);               \
        pic_select_bank_status_ |= (uint8_t)(((bank) & 0x03U) << 5);   \
        EPIC_REG8(PIC_REG_STATUS) = pic_select_bank_status_;           \
    } while (0)

#endif /* PIC16F87XA_SFR_H */
