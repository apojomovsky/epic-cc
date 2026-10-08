; menu-demo.asm @ 826e77c: retval MOVFF sites
; each block below is verbatim from the source listing;
; block i covers source lines lo-hi (1-based) as noted.
; --- source lines 56-89 ---
    org 0x0008
PIC18_IRQ_Handler:
    MOVFF 0xFF6, 0x005
    MOVFF 0xFF7, 0x006
    MOVFF 0xFF8, 0x007
    MOVFF 0xFEA, 0x004
    MOVFF 0xFD8, 0x009
    MOVFF 0xFE0, 0x00A
    MOVFF 0xFE9, 0x00B
    MOVFF 0xFE1, 0x0CA
    MOVFF 0xFE2, 0x0CB
    MOVFF 0xFF3, 0x0CC
    MOVFF 0xFF4, 0x0CD
    MOVFF 0xFF5, 0x0CE
    MOVFF 0xFFA, 0x0CF
    MOVFF 0xFFB, 0x0D0
    MOVFF 0x000, 0x00C
    MOVFF 0x001, 0x00D
    MOVFF 0x002, 0x00E
    MOVFF 0x003, 0x00F
    MOVWF 0x008,A
    CALL epic_dispatch_all_irqs_isr
    MOVFF 0x007, 0xFF8
    MOVFF 0x006, 0xFF7
    MOVFF 0x005, 0xFF6
    MOVFF 0x004, 0xFEA
    MOVFF 0x00B, 0xFE9
    MOVFF 0x00A, 0xFE0
    MOVFF 0x00F, 0x003
    MOVFF 0x00E, 0x002
    MOVFF 0x00D, 0x001
    MOVFF 0x00C, 0x000
    MOVFF 0x0CA, 0xFE1
    MOVFF 0x0CB, 0xFE2
; --- source lines 499-537 ---
    MOVFF 0x09D, 0x095
    MOVFF 0x09E, 0x096
tmp46:
    CLRF 0x097,B
    CLRF 0x098,B
    MOVLW 0x80
    MOVWF 0x0A6,B
    MOVLW 0x25
    MOVWF 0x0A7,B
    CLRF 0x0A8,B
    CLRF 0x0A9,B
    MOVFF 0x095, 0x0AA
    MOVFF 0x096, 0x0AB
    MOVFF 0x097, 0x0AC
    MOVFF 0x098, 0x0AD
    CALL __shl_u32
    MOVFF 0x000, 0x099
    MOVFF 0x001, 0x09A
    MOVFF 0x002, 0x09B
    MOVFF 0x003, 0x09C
    MOVLB 0x0
    CLRF 0x0A6,B
    MOVLW 0x6C
    MOVWF 0x0A7,B
    MOVLW 0xDC
    MOVWF 0x0A8,B
    MOVLW 0x02
    MOVWF 0x0A9,B
    MOVFF 0x099, 0x0AA
    MOVFF 0x09A, 0x0AB
    MOVFF 0x09B, 0x0AC
    MOVFF 0x09C, 0x0AD
    CALL __udiv_u32
    MOVFF 0x000, 0x095
    MOVFF 0x001, 0x096
    MOVFF 0x002, 0x097
    MOVFF 0x003, 0x098
    MOVLW 0xFF
    MOVLB 0x0
; --- source lines 1568-1586 ---
__pa75:
    BCF 0xFD8,0,A
    RRCF 0x056,F,A
    RETURN
epic_dispatch_all_irqs:
    MOVFF 0xFF2, 0x04D
    MOVLW 0x04
    ANDWF 0x04D,W,A
    MOVWF 0x04F,A
    MOVF 0x04F,W,A
    BNZ epic_dispatch_all_irqs_L4
    BRA epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_L4:
    MOVLW 0x04
    RCALL __pa71
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x04F
    MOVF 0x04F,W,A
    BNZ epic_dispatch_all_irqs_L7
; --- source lines 1620-1638 ---
    BRA tmp313
tmp311:
    BRA epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit:
    MOVFF 0xF9E, 0x04D
    CLRF 0x04E,A
    MOVLW 0x02
    RCALL __pa41
    BNZ epic_dispatch_all_irqs_L18
    MOVF 0x050,W,A
    BNZ epic_dispatch_all_irqs_L18
    BRA epic_dispatch_all_irqs_LTIMER2_IRQHandler.exit
epic_dispatch_all_irqs_L18:
    MOVLW 0x06
    RCALL __pa71
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x04F
    MOVF 0x04F,W,A
    BNZ epic_dispatch_all_irqs_L21
; --- source lines 1702-1720 ---
epic_dispatch_all_irqs_LTIMER2_IRQHandler.exit:
    MOVLW 0x10
    RCALL __pa41
    BNZ epic_dispatch_all_irqs_L30
    MOVF 0x050,W,A
    BNZ epic_dispatch_all_irqs_L30
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_L30:
    MOVFF 0xF9D, 0x04F
    RCALL __pa86
    BNZ epic_dispatch_all_irqs_L34
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_L34:
    MOVLW 0x0A
    RCALL __pa71
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x04F
    CLRF 0x051,A
    MOVF 0x04F,W,A
; --- source lines 1763-1781 ---
tmp329:
tmp330:
    BRA tmp330
tmp328:
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit:
    MOVLW 0x20
    RCALL __pa41
    BNZ epic_dispatch_all_irqs_L46
    MOVF 0x050,W,A
    BNZ epic_dispatch_all_irqs_L46
    BRA epic_dispatch_all_irqs_LUSART_RX_IRQHandler.exit
epic_dispatch_all_irqs_L46:
    MOVLW 0x0B
    RCALL __pa71
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x04D
    MOVF 0x04D,W,A
    BNZ epic_dispatch_all_irqs_L49
; --- source lines 1820-1838 ---
    MOVFF 0xFA1, 0x04F
    RCALL __pa86
    BNZ epic_dispatch_all_irqs_L60
    BRA epic_dispatch_all_irqs_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_L60:
    MOVFF 0xFA0, 0x04D
    MOVLW 0x10
    ANDWF 0x04D,W,A
    MOVWF 0x04F,A
    MOVF 0x04F,W,A
    BNZ epic_dispatch_all_irqs_L64
    BRA epic_dispatch_all_irqs_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_L64:
    MOVLW 0x0F
    RCALL __pa71
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x04D
    MOVF 0x04D,W,A
    BNZ epic_dispatch_all_irqs_L67
; --- source lines 1939-1957 ---
    MOVFF 0x01B, 0xFEA
    MOVFF 0xFEF, 0x01D
    MOVF 0x01D,W,A
    BNZ tmp114
    BRA epic_harness_log_L.loopexit
tmp114:
    MOVFF 0x01D, 0x01E
    MOVF 0x01A,W,A
    MOVWF 0x01F,A
    MOVF 0x01B,W,A
    MOVWF 0x020,A
    BRA epic_harness_log_L.preheader
epic_harness_log_L.preheader:
    BRA epic_harness_log_L6
epic_harness_log_L6:
    CALL EPIC_USART_IsTxShiftRegisterEmpty
    MOVFF 0x000, 0x01A
    MOVF 0x01A,W,A
    BNZ epic_harness_log_L9
; --- source lines 1969-1987 ---
    BNZ tmp115
    BRA epic_harness_log_L.loopexit
tmp115:
    MOVFF 0x01A, 0x01E
    MOVF 0x01F,W,A
    ADDLW 0x01
    MOVWF 0x01F,A
    MOVF 0x020,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x020,A
    BRA epic_harness_log_L.preheader
epic_harness_log_L.loopexit:
    RETURN
epic_taskmgr_init:
    CALL EPIC_IRQ_Disable
    MOVFF 0x000, 0x01A
    CLRF 0x01B,A
    CLRF 0x01C,A
; --- source lines 2453-2477 ---
epic_taskmgr_run_L.loopexit:
; --- asm start ---
clrwdt
; --- asm end ---
    MOVLW 0x01
    ADDWF 0x01A,W,A
    MOVWF 0x020,A
    MOVLW 0x00
    ADDWFC 0x01B,W,A
    MOVWF 0x021,A
    MOVLW 0x00
    ADDWFC 0x01C,W,A
    MOVWF 0x022,A
    MOVLW 0x00
    ADDWFC 0x01D,W,A
    MOVWF 0x023,A
    MOVFF 0x020, 0x034
    MOVFF 0x021, 0x035
    MOVFF 0x022, 0x036
    MOVWF 0x037,A
    CALL epic_harness_running
    MOVFF 0x000, 0x024
    MOVFF 0x001, 0x025
    MOVF 0x024,W,A
    BNZ tmp384
; --- source lines 2822-2841 ---
    BRA epic_lcd_clear
epic_lcd_home:
    BRA epic_lcd_home
epic_lcd_set_cursor:
    RETURN
epic_lcd_write_char:
    BRA epic_lcd_write_char
epic_lcd_write:
    BRA epic_lcd_write
epic_lcd_print:
    MOVLB 0x0
    MOVLW 0xAC
    MOVWF 0x0C5,B
    MOVLW 0x02
    MOVWF 0x0C6,B
    CALL strlen
    MOVFF 0x000, 0x0C3
    MOVFF 0x001, 0x0C4
    RETURN
epic_lcd_display_on:
; --- source lines 3064-3085 ---
    SUBWFB 0x017,W,A
    BNC gpio4_delay_us_L6
    BRA gpio4_delay_us_L4
gpio4_delay_us_L4:
    MOVFF 0x014, 0x0A6
    MOVFF 0x015, 0x0A7
    MOVFF 0x016, 0x0A8
    MOVFF 0x017, 0x0A9
    MOVLB 0x0
    MOVLW 0xE8
    MOVWF 0x0AA,B
    MOVLW 0x03
    MOVWF 0x0AB,B
    CLRF 0x0AC,B
    CLRF 0x0AD,B
    CALL __udiv_u32
    MOVFF 0x000, 0x010
    MOVFF 0x001, 0x011
    MOVFF 0x002, 0x012
    MOVFF 0x003, 0x013
    MOVFF 0x010, 0x019
    MOVFF 0x011, 0x01A
; --- source lines 3272-3300 ---
    MOVFF 0x03B, 0x03F
    MOVFF 0x03C, 0x040
    CLRF 0x041,A
    BRA epic_serial_put_u16_L3
epic_serial_put_u16_L3:
    MOVFF 0x03D, 0x0A6
    MOVFF 0x03E, 0x0A7
    MOVFF 0x03F, 0x0A8
    MOVFF 0x040, 0x0A9
    MOVLB 0x0
    MOVLW 0x0A
    MOVWF 0x0AA,B
    CLRF 0x0AB,B
    CLRF 0x0AC,B
    CLRF 0x0AD,B
    CALL __udiv_u32
    MOVFF 0x000, 0x044
    MOVFF 0x001, 0x045
    MOVFF 0x002, 0x046
    MOVFF 0x003, 0x047
    MOVFF 0x044, 0x048
    MOVFF 0x048, 0x0BF
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0C0,B
    CALL __mul_u8
    MOVFF 0x000, 0x04A
    MOVFF 0x03D, 0x048
    MOVF 0x048,W,A
; --- source lines 3563-3598 ---
    MOVFF 0x044, 0x036
    MOVFF 0x045, 0x037
    MOVFF 0x03E, 0x030
    MOVFF 0x03F, 0x031
    BRA menu_demo_init_L18
menu_demo_init_L31:
    MOVFF 0x049, 0x095
    MOVFF 0x04A, 0x096
    MOVFF 0x04B, 0x097
    MOVFF 0x04C, 0x098
    MOVFF 0x038, 0x099
    MOVFF 0x039, 0x09A
    MOVFF 0x03A, 0x09B
    MOVFF 0x03B, 0x09C
    CALL __mul_u32
    CALL __pa43
    MOVFF 0x002, 0x01C
    MOVFF 0x003, 0x01D
    MOVLB 0x0
    MOVLW 0xE0
    MOVWF 0x0A6,B
    MOVLW 0x2E
    MOVWF 0x0A7,B
    CLRF 0x0A8,B
    CLRF 0x0A9,B
    MOVFF 0x01A, 0x0AA
    MOVFF 0x01B, 0x0AB
    MOVFF 0x01C, 0x0AC
    MOVFF 0x01D, 0x0AD
    CALL __udiv_u32
    MOVFF 0x000, 0x01E
    MOVFF 0x001, 0x01F
    MOVFF 0x002, 0x020
    MOVFF 0x003, 0x021
    MOVFF 0x049, 0x024
    MOVFF 0x04A, 0x025
; --- source lines 3689-3710 ---
    SUBWFB 0x035,W,A
    BNC tmp223
    BRA menu_demo_init_L53
tmp223:
    CALL __pa25
    BRA menu_demo_init_L64
menu_demo_init_L53:
    MOVFF 0x028, 0x095
    MOVFF 0x029, 0x096
    MOVFF 0x02A, 0x097
    MOVFF 0x02B, 0x098
    MOVFF 0x01A, 0x099
    MOVFF 0x01B, 0x09A
    MOVFF 0x01C, 0x09B
    MOVFF 0x01D, 0x09C
    CALL __mul_u32
    MOVFF 0x000, 0x032
    MOVFF 0x001, 0x033
    MOVFF 0x002, 0x034
    MOVFF 0x003, 0x035
    CLRF 0x03C,A
    MOVLW 0xE0
; --- source lines 3863-3888 ---
tmp231:
    CALL __pa69
    DECFSZ 0xFE8,F,A
    BRA tmp231
    MOVFF 0x046, 0x05E
    MOVFF 0x047, 0x05F
    MOVFF 0x044, 0x060
    MOVFF 0x045, 0x061
    MOVFF 0x048, 0x062
    MOVLW LOW(epic_tick_on_overflow)
    MOVLB 0x0
    MOVWF 0x063,B
    MOVLW HIGH(epic_tick_on_overflow)
    MOVWF 0x064,B
    MOVLW 0x5E
    CALL __pa93
    CALL EPIC_TIMER2_Init
    MOVFF 0x000, 0x02C
    MOVFF 0x001, 0x02D
    MOVLW 0x5E
    CALL __pa93
    CALL EPIC_TIMER2_Start
    MOVFF 0x000, 0x02C
    MOVFF 0x001, 0x02D
    MOVLW 0x01
    MOVWF 0x095,B
; --- source lines 4190-4214 ---
    MOVLW UPPER(__const.menu_demo_init.t2)
    MOVWF 0xF8,A
    LFSR 1, 0x078
    MOVLW 0x07
tmp243:
    CALL __pa69
    DECFSZ 0xFE8,F,A
    BRA tmp243
    MOVLB 0x0
    MOVLW 0x02
    MOVWF 0x078,B
    CLRF 0x079,B
    SETF 0x07C,B
    MOVLW 0x78
    CALL __pa93
    CALL EPIC_TIMER2_Init
    MOVFF 0x000, 0x022
    MOVFF 0x001, 0x023
    MOVLW 0x78
    CALL __pa93
    CALL EPIC_TIMER2_Start
    MOVFF 0x000, 0x022
    MOVFF 0x001, 0x023
    MOVLW 0x01
    MOVWF 0x07F,B
; --- source lines 4608-4636 ---
    SWAPF 0x024,W,A
    ANDLW 0xF0
    MOVWF 0x024,A
    MOVLW 0x70
    ANDWF 0x024,W,A
    MOVWF 0x01A,A
    IORWF 0x01E,W,A
    MOVWF 0x024,A
    MOVF 0x036,W,A
    IORWF 0x024,W,A
    MOVWF 0x01A,A
    MOVFF 0x01A, 0xFB6
    BRA menu_demo_init_LEPIC_CCP_Init.exit
menu_demo_init_LEPIC_CCP_Init.exit:
    CLRF 0x095,B
    CALL EPIC_EEPROM_ReadByte
    MOVFF 0x000, 0x028
    MOVLW 0xE9
    SUBWF 0x028,W,A
    BNZ menu_demo_init_L284
    BRA menu_demo_init_L281
menu_demo_init_L281:
    MOVLB 0x0
    MOVLW 0x01
    MOVWF 0x095,B
    CALL EPIC_EEPROM_ReadByte
    MOVFF 0x000, 0x01A
    CLRF 0x01E,A
    MOVLW 0x0A
; --- source lines 4984-5010 ---
    MOVLW 0xBB
    MOVWF 0x09B,B
    MOVLW 0x02
    MOVWF 0x09C,B
    CLRF 0x09D,B
    CLRF 0x09E,B
    RCALL __pa100
    BRA redraw_L41
redraw_L41:
    MOVFF 0x09F, 0x0BF
    MOVFF 0x0A0, 0x0C0
    MOVLB 0x0
    MOVLW 0x0A
    MOVWF 0x0C1,B
    CLRF 0x0C2,B
    CALL __udiv_u16
    MOVFF 0x000, 0x095
    MOVFF 0x001, 0x096
    MOVFF 0x095, 0x0A1
    MOVFF 0x0A1, 0x0BF
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0C0,B
    CALL __mul_u8
    MOVFF 0x000, 0x0A3
    MOVFF 0x09F, 0x0A1
    MOVLB 0x0
; --- source lines 5262-5280 ---
    BRA redraw_status_L11
redraw_status_L11:
    MOVFF 0x0B6, 0x0BF
    MOVFF 0x0B7, 0x0C0
    MOVLB 0x0
    MOVLW 0x0A
    MOVWF 0x0C1,B
    CLRF 0x0C2,B
    CALL __udiv_u16
    RCALL __pa84
    MOVFF 0x0A8, 0x0B8
    MOVFF 0x0B8, 0x0BF
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0C0,B
    CALL __mul_u8
    MOVFF 0x000, 0x0BA
    MOVFF 0x0B6, 0x0B8
    MOVLB 0x0
; --- source lines 5502-5521 ---
    BNZ tmp188
    MOVLW 0xFF
    SUBWF 0x0AB,W,B
    BNZ tmp188
    BRA redraw_status_L38
tmp188:
    MOVFF 0x0AA, 0x0A8
    MOVFF 0x0AB, 0x0A9
    BRA redraw_status_L36
redraw_status_L38:
    CALL __pa13
    CALL epic_lcd_print
    RETURN
menu_demo_task_adc:
    BRA menu_demo_task_adc
__pa84:
    MOVFF 0x000, 0x0A8
    MOVFF 0x001, 0x0A9
    RETURN
__pa99:
; --- source lines 5618-5637 ---
    MOVF 0x047,W,A
    BNZ menu_demo_task_ui_L36
    BRA menu_demo_task_ui_L33
menu_demo_task_ui_L33:
    MOVLW 0x01
    ADDWF 0x03E,W,A
    MOVWF 0x048,A
    MOVLW 0x00
    ADDWFC 0x03F,W,A
    MOVWF 0x049,A
    MOVFF 0x048, 0x04F
    MOVWF 0x050,A
    MOVLW 0x03
    MOVWF 0x051,A
    CLRF 0x052,A
    CALL __urem_u16
    MOVFF 0x000, 0x04A
    MOVFF 0x001, 0x04B
    MOVFF 0x04A, 0x266
    MOVFF 0x04B, 0x267
; --- source lines 5757-5794 ---
    MOVFF 0x265, 0x034
    MOVF 0x034,W,A
    BZ menu_demo_task_eeprom_L23
    BRA menu_demo_task_eeprom_L5
menu_demo_task_eeprom_L5:
    CLRF 0x03A,A
    MOVLW 0xE9
    MOVWF 0x03B,A
    CALL EPIC_EEPROM_WriteByte
    CALL __pa26
    MOVLW 0x01
    MOVWF 0x034,A
    CLRF 0x035,A
    BRA menu_demo_task_eeprom_L21
menu_demo_task_eeprom_L7:
    CALL EPIC_EEPROM_IsWriteComplete
    MOVFF 0x000, 0x036
    MOVF 0x036,W,A
    BNZ menu_demo_task_eeprom_L10
    BRA menu_demo_task_eeprom_L23
menu_demo_task_eeprom_L10:
    RCALL __pa40
    MOVFF 0x264, 0x036
    MOVLW 0x01
    MOVWF 0x03A,A
    MOVFF 0x036, 0x03B
    CALL EPIC_EEPROM_WriteByte
    MOVFF 0x000, 0x038
    MOVFF 0x001, 0x039
    MOVLW 0x02
    MOVWF 0x034,A
    CLRF 0x035,A
    BRA menu_demo_task_eeprom_L21
menu_demo_task_eeprom_L15:
    CALL EPIC_EEPROM_IsWriteComplete
    MOVFF 0x000, 0x036
    MOVF 0x036,W,A
    BNZ menu_demo_task_eeprom_L18
; --- source lines 5817-5835 ---
    MOVLW 0x00
    ADDWFC 0x037,W,A
    MOVWF 0x06B,B
    RETURN
menu_demo_task_heartbeat:
    MOVFF 0x264, 0x036
    MOVFF 0xFBD, 0x034
    MOVLW 0xCF
    ANDWF 0x034,W,A
    MOVWF 0x038,A
    MOVFF 0x038, 0xFBD
    MOVFF 0x036, 0x0BF
    MOVLB 0x0
    MOVLW 0x19
    MOVWF 0x0C0,B
    CALL __mul_u8
    MOVFF 0x000, 0x034
    MOVFF 0x034, 0xFBE
    CLRF 0x034,A
; --- source lines 6091-6140 ---
main:
    MOVLW 0xC8
    MOVWF 0x01A,A
    CLRF 0x01B,A
    CLRF 0x01C,A
    CLRF 0x01D,A
    CALL epic_harness_init
    CALL menu_demo_init
    CALL epic_taskmgr_init
    MOVLW LOW(task_stimulus)
    MOVWF 0x01A,A
    MOVLW HIGH(task_stimulus)
    RCALL __pa55
    MOVLW 0x01
    RCALL __pa92
    CLRF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(menu_demo_task_ui)
    MOVWF 0x01A,A
    MOVLW HIGH(menu_demo_task_ui)
    RCALL __pa55
    MOVLW 0x01
    RCALL __pa92
    MOVLW 0x02
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(menu_demo_task_eeprom)
    MOVWF 0x01A,A
    MOVLW HIGH(menu_demo_task_eeprom)
    RCALL __pa55
    MOVLW 0x05
    RCALL __pa92
    MOVLW 0x03
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(menu_demo_task_heartbeat)
    MOVWF 0x01A,A
    MOVLW HIGH(menu_demo_task_heartbeat)
    RCALL __pa55
    MOVLW 0x32
    RCALL __pa92
    MOVLW 0x04
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    CLRF 0x01A,A
    MOVLW 0x03
; --- source lines 6264-6282 ---
main_L32:
    CALL menu_demo_screen
    RCALL __pa46
    MOVF 0x016,W,A
    BNZ main_L35
    MOVF 0x017,W,A
    BNZ main_L35
    BRA main_L38
main_L35:
    RCALL __pa3
    MOVLW 0x31
    RCALL __pa7
    CALL epic_harness_log
    BRA main_L38
main_L38:
    CALL menu_demo_brightness
    MOVFF 0x000, 0x016
    MOVLW 0x07
    SUBWF 0x016,W,A
; --- source lines 6454-6473 ---
    MOVLW 0x20
    MOVWF 0x0D6,B
    MOVLW 0xD2
    MOVWF 0x01A,A
    MOVLW 0x02
    MOVWF 0x01B,A
    RETURN
__pa29:
    MOVLW LOW(log_fire_ticks.hx)
    MOVWF 0xF6,A
    MOVLW HIGH(log_fire_ticks.hx)
    MOVWF 0xF7,A
    MOVLW UPPER(log_fire_ticks.hx)
    MOVWF 0xF8,A
    RETURN
__pa46:
    MOVFF 0x000, 0x016
    MOVFF 0x001, 0x017
    RETURN
__pa49:
; --- source lines 6480-6499 ---
__pa55:
    MOVWF 0x01B,A
    CLRF 0x01C,A
    CLRF 0x01D,A
    RETURN
__pa73:
    MOVWF 0x01A,A
    MOVLW 0x02
    MOVWF 0x01B,A
    RETURN
__pa92:
    MOVWF 0x01E,A
    CLRF 0x01F,A
    RETURN
task_stimulus:
    CALL epic_taskmgr_ticks
    MOVFF 0x000, 0x036
    MOVFF 0x001, 0x037
    MOVFF 0x2BD, 0x034
    CLRF 0x038,A
; --- source lines 7101-7119 ---
    MOVWF 0x000,A
    RETURN
epic_dispatch_all_irqs_isr:
    MOVFF 0xFF2, 0x0D1
    MOVLW 0x04
    MOVLB 0x0
    ANDWF 0x0D1,W,B
    MOVWF 0x0D3,B
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L4
    BRA epic_dispatch_all_irqs_isr_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_isr_L4:
    MOVLW 0x04
    MOVWF 0x0D7,B
    CLRF 0x0D8,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D3
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L7
; --- source lines 7158-7176 ---
    MOVLW 0x02
    ANDWF 0x0D1,W,B
    MOVWF 0x0D3,B
    MOVLW 0x00
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L18
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L18
    BRA epic_dispatch_all_irqs_isr_LTIMER2_IRQHandler.exit
epic_dispatch_all_irqs_isr_L18:
    MOVLW 0x06
    MOVWF 0x0D7,B
    CLRF 0x0D8,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D3
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L21
; --- source lines 7232-7250 ---
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L30
    BRA epic_dispatch_all_irqs_isr_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L30:
    MOVFF 0xF9D, 0x0D3
    MOVLW 0x10
    ANDWF 0x0D3,W,B
    MOVWF 0x0D5,B
    MOVF 0x0D5,W,B
    BNZ epic_dispatch_all_irqs_isr_L34
    BRA epic_dispatch_all_irqs_isr_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L34:
    MOVLW 0x0A
    MOVWF 0x0D7,B
    CLRF 0x0D8,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D3
    CLRF 0x0D5,B
    MOVF 0x0D3,W,B
; --- source lines 7301-7319 ---
    MOVLB 0x0
    ANDWF 0x0D1,W,B
    MOVWF 0x0D3,B
    MOVLW 0x00
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L46
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L46
    BRA epic_dispatch_all_irqs_isr_LUSART_RX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L46:
    MOVLW 0x0B
    MOVWF 0x0D7,B
    CLRF 0x0D8,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D1
    MOVF 0x0D1,W,B
    BNZ epic_dispatch_all_irqs_isr_L49
; --- source lines 7363-7381 ---
    MOVF 0x0D5,W,B
    BNZ epic_dispatch_all_irqs_isr_L60
    BRA epic_dispatch_all_irqs_isr_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_isr_L60:
    MOVFF 0xFA0, 0x0D1
    MOVLW 0x10
    ANDWF 0x0D1,W,B
    MOVWF 0x0D3,B
    MOVF 0x0D3,W,B
    BNZ epic_dispatch_all_irqs_isr_L64
    BRA epic_dispatch_all_irqs_isr_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_isr_L64:
    MOVLW 0x0F
    MOVWF 0x0D7,B
    CLRF 0x0D8,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D1
    MOVF 0x0D1,W,B
    BNZ epic_dispatch_all_irqs_isr_L67
; --- source lines 7569-7590 ---
    MOVF 0x0AA,W,B
    ANDLW 0x1F
    MOVWF 0x0AE,B
    MOVF 0x0AE,F,B
    BTFSC 0xFD8,2,A
    BRA tmp415
tmp414:
    BCF 0xFD8,0,A
    MOVLB 0x0
    RLCF 0x0A6,F,B
    RLCF 0x0A7,F,B
    RLCF 0x0A8,F,B
    RLCF 0x0A9,F,B
    DECFSZ 0x0AE,F,B
    BRA tmp414
tmp415:
    MOVFF 0x0A6, 0x000
    MOVFF 0x0A7, 0x001
    MOVFF 0x0A8, 0x002
    MOVFF 0x0A9, 0x003
    RETURN
__udiv_u32:
; --- source lines 7616-7714 ---
    SUBWFB 0x0B1,F,B
    BNC tmp417
    BSF 0x0A6,0,B
    BRA tmp418
tmp417:
    MOVF 0x0AA,W,B
    ADDWF 0x0AE,F,B
    MOVF 0x0AB,W,B
    ADDWFC 0x0AF,F,B
    MOVF 0x0AC,W,B
    ADDWFC 0x0B0,F,B
    MOVF 0x0AD,W,B
    ADDWFC 0x0B1,F,B
tmp418:
    DECFSZ 0x0B2,F,B
    BRA tmp416
    MOVFF 0x0A6, 0x000
    MOVFF 0x0A7, 0x001
    MOVFF 0x0A8, 0x002
    MOVFF 0x0A9, 0x003
    RETURN
__mul_u8:
    MOVLB 0x0
    MOVF 0x0BF,W,B
    MULWF 0x0C0,B
    MOVFF 0xFF3, 0x000
    RETURN
__mul_u32:
    MOVLB 0x0
    CLRF 0x09D,B
    CLRF 0x09E,B
    CLRF 0x09F,B
    CLRF 0x0A0,B
    MOVF 0x095,W,B
    MULWF 0x099,B
    MOVF 0xFF3,W,A
    ADDWF 0x09D,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x09E,F,B
    MOVF 0x096,W,B
    MULWF 0x099,B
    MOVF 0xFF3,W,A
    ADDWF 0x09E,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x09F,F,B
    MOVF 0x097,W,B
    MULWF 0x099,B
    MOVF 0xFF3,W,A
    ADDWF 0x09F,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A0,F,B
    MOVF 0x098,W,B
    MULWF 0x099,B
    MOVF 0xFF3,W,A
    ADDWF 0x0A0,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A1,F,B
    MOVF 0x095,W,B
    MULWF 0x09A,B
    MOVF 0xFF3,W,A
    ADDWF 0x09E,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x09F,F,B
    MOVF 0x096,W,B
    MULWF 0x09A,B
    MOVF 0xFF3,W,A
    ADDWF 0x09F,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A0,F,B
    MOVF 0x097,W,B
    MULWF 0x09A,B
    MOVF 0xFF3,W,A
    ADDWF 0x0A0,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A1,F,B
    MOVF 0x095,W,B
    MULWF 0x09B,B
    MOVF 0xFF3,W,A
    ADDWF 0x09F,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A0,F,B
    MOVF 0x096,W,B
    MULWF 0x09B,B
    MOVF 0xFF3,W,A
    ADDWF 0x0A0,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A1,F,B
    MOVF 0x095,W,B
    MULWF 0x09C,B
    MOVF 0xFF3,W,A
    ADDWF 0x0A0,F,B
    MOVF 0xFF4,W,A
    ADDWFC 0x0A1,F,B
    MOVFF 0x09D, 0x000
    MOVFF 0x09E, 0x001
    MOVFF 0x09F, 0x002
    MOVFF 0x0A0, 0x003
    RETURN
__udiv_u16:
; --- source lines 7726-7745 ---
    RLCF 0x0C4,F,B
    MOVF 0x0C1,W,B
    SUBWF 0x0C3,F,B
    MOVF 0x0C2,W,B
    SUBWFB 0x0C4,F,B
    BNC tmp420
    BSF 0x0BF,0,B
    BRA tmp421
tmp420:
    MOVF 0x0C1,W,B
    ADDWF 0x0C3,F,B
    MOVF 0x0C2,W,B
    ADDWFC 0x0C4,F,B
tmp421:
    DECFSZ 0x0C5,F,B
    BRA tmp419
    MOVFF 0x0BF, 0x000
    MOVFF 0x0C0, 0x001
    RETURN
__urem_u16:
; --- source lines 7755-7774 ---
    RLCF 0x054,F,A
    MOVF 0x051,W,A
    SUBWF 0x053,F,A
    MOVF 0x052,W,A
    SUBWFB 0x054,F,A
    BNC tmp423
    BSF 0x04F,0,A
    BRA tmp424
tmp423:
    MOVF 0x051,W,A
    ADDWF 0x053,F,A
    MOVF 0x052,W,A
    ADDWFC 0x054,F,A
tmp424:
    DECFSZ 0x055,F,A
    BRA tmp422
    MOVFF 0x053, 0x000
    MOVFF 0x054, 0x001
    RETURN
__start:
; --- source lines 8046-8065 ---
__pa23:
    MOVFF 0x010, 0xFE9
    MOVFF 0x011, 0xFEA
    RETURN
__pa25:
    MOVFF 0x05B, 0x044
    MOVFF 0x05C, 0x045
    MOVFF 0x059, 0x046
    MOVFF 0x05A, 0x047
    MOVFF 0x03E, 0x048
    MOVFF 0x024, 0x051
    MOVFF 0x025, 0x052
    MOVFF 0x026, 0x053
    MOVFF 0x027, 0x054
    RETURN
__pa26:
    MOVFF 0x000, 0x034
    MOVFF 0x001, 0x035
    RETURN
__pa27:
; --- source lines 8105-8124 ---
    RETURN
__pa42:
    MOVLW 0xFF
    MOVLB 0x0
    ADDWF 0x095,W,B
    MOVWF 0x09B,B
    MOVLW 0xFF
    ADDWFC 0x096,W,B
    MOVWF 0x09C,B
    CLRF 0x09D,B
    MOVLW 0x04
    SUBWF 0x09B,W,B
    MOVLW 0x00
    SUBWFB 0x09C,W,B
    RETURN
__pa43:
    MOVFF 0x000, 0x01A
    MOVFF 0x001, 0x01B
    RETURN
__pa45:
