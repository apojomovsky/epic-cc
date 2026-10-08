; control-demo.asm @ 826e77c: retval MOVFF sites
; each block below is verbatim from the source listing;
; block i covers source lines lo-hi (1-based) as noted.
; --- source lines 84-117 ---
    org 0x0008
PIC18_IRQ_Handler:
    MOVFF 0xFF6, 0x005
    MOVFF 0xFF7, 0x006
    MOVFF 0xFF8, 0x007
    MOVFF 0xFEA, 0x004
    MOVFF 0xFD8, 0x009
    MOVFF 0xFE0, 0x00A
    MOVFF 0xFE9, 0x00B
    MOVFF 0xFE1, 0x0CB
    MOVFF 0xFE2, 0x0CC
    MOVFF 0xFF3, 0x0CD
    MOVFF 0xFF4, 0x0CE
    MOVFF 0xFF5, 0x0CF
    MOVFF 0xFFA, 0x0D0
    MOVFF 0xFFB, 0x0D1
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
    MOVFF 0x0CB, 0xFE1
    MOVFF 0x0CC, 0xFE2
; --- source lines 216-254 ---
    MOVFF 0x05C, 0x054
    MOVFF 0x05D, 0x055
tmp11:
    CLRF 0x056,A
    CLRF 0x057,A
    MOVLW 0x80
    MOVWF 0x065,B
    MOVLW 0x25
    MOVWF 0x066,B
    CLRF 0x067,B
    CLRF 0x068,B
    MOVFF 0x054, 0x069
    MOVFF 0x055, 0x06A
    MOVFF 0x056, 0x06B
    MOVFF 0x057, 0x06C
    CALL __shl_u32
    MOVFF 0x000, 0x058
    MOVFF 0x001, 0x059
    MOVFF 0x002, 0x05A
    MOVFF 0x003, 0x05B
    MOVLB 0x0
    CLRF 0x0B9,B
    MOVLW 0x6C
    MOVWF 0x0BA,B
    MOVLW 0xDC
    MOVWF 0x0BB,B
    MOVLW 0x02
    MOVWF 0x0BC,B
    MOVFF 0x058, 0x0BD
    MOVFF 0x059, 0x0BE
    MOVFF 0x05A, 0x0BF
    MOVFF 0x05B, 0x0C0
    CALL __udiv_u32
    MOVFF 0x000, 0x054
    MOVFF 0x001, 0x055
    MOVFF 0x002, 0x056
    MOVFF 0x003, 0x057
    MOVLW 0xFF
    ADDWF 0x054,W,A
; --- source lines 1290-1308 ---
    MOVFF 0x0C2, 0x0BF
    RETURN
epic_dispatch_all_irqs:
    MOVFF 0xFF2, 0x0B9
    MOVLW 0x04
    MOVLB 0x0
    ANDWF 0x0B9,W,B
    MOVWF 0x0BB,B
    MOVF 0x0BB,W,B
    BNZ epic_dispatch_all_irqs_L4
    BRA epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_L4:
    MOVLW 0x04
    MOVWF 0x0BF,B
    CLRF 0x0C0,B
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x0BB
    MOVF 0x0BB,W,B
    BNZ epic_dispatch_all_irqs_L7
; --- source lines 1342-1360 ---
tmp259:
    BRA epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_LTIMER0_IRQHandler.exit:
    MOVFF 0xF9E, 0x0B9
    CLRF 0x0BA,B
    MOVLW 0x02
    RCALL __pa37
    BNZ epic_dispatch_all_irqs_L18
    MOVF 0x0BC,W,B
    BNZ epic_dispatch_all_irqs_L18
    BRA epic_dispatch_all_irqs_LTIMER2_IRQHandler.exit
epic_dispatch_all_irqs_L18:
    MOVLW 0x06
    MOVWF 0x0BF,B
    CLRF 0x0C0,B
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x0BB
    MOVF 0x0BB,W,B
    BNZ epic_dispatch_all_irqs_L21
; --- source lines 1411-1429 ---
    MOVF 0x0BC,W,B
    BNZ epic_dispatch_all_irqs_L30
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_L30:
    MOVFF 0xF9D, 0x0BB
    MOVLW 0x10
    ANDWF 0x0BB,W,B
    MOVWF 0x0BD,B
    MOVF 0x0BD,W,B
    BNZ epic_dispatch_all_irqs_L34
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_L34:
    MOVLW 0x0A
    MOVWF 0x0BF,B
    CLRF 0x0C0,B
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x0BB
    CLRF 0x0BD,B
    MOVF 0x0BB,W,B
; --- source lines 1474-1492 ---
    BRA tmp276
tmp274:
    BRA epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_LUSART_TX_IRQHandler.exit:
    MOVLW 0x20
    MOVLB 0x0
    RCALL __pa37
    BNZ epic_dispatch_all_irqs_L46
    MOVF 0x0BC,W,B
    BNZ epic_dispatch_all_irqs_L46
    BRA epic_dispatch_all_irqs_LUSART_RX_IRQHandler.exit
epic_dispatch_all_irqs_L46:
    MOVLW 0x0B
    MOVWF 0x0BF,B
    CLRF 0x0C0,B
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x0B9
    MOVF 0x0B9,W,B
    BNZ epic_dispatch_all_irqs_L49
; --- source lines 1535-1553 ---
    MOVF 0x0BD,W,B
    BNZ epic_dispatch_all_irqs_L60
    BRA epic_dispatch_all_irqs_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_L60:
    MOVFF 0xFA0, 0x0B9
    MOVLW 0x10
    ANDWF 0x0B9,W,B
    MOVWF 0x0BB,B
    MOVF 0x0BB,W,B
    BNZ epic_dispatch_all_irqs_L64
    BRA epic_dispatch_all_irqs_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_L64:
    MOVLW 0x0F
    MOVWF 0x0BF,B
    CLRF 0x0C0,B
    CALL EPIC_IRQ_GetFlag
    MOVFF 0x000, 0x0B9
    MOVF 0x0B9,W,B
    BNZ epic_dispatch_all_irqs_L67
; --- source lines 1647-1665 ---
    MOVFF 0x01B, 0xFEA
    MOVFF 0xFEF, 0x01D
    MOVF 0x01D,W,A
    BNZ tmp79
    BRA epic_harness_log_L.loopexit
tmp79:
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
; --- source lines 1680-1698 ---
    BNZ tmp80
    BRA epic_harness_log_L.loopexit
tmp80:
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
; --- source lines 2168-2192 ---
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
    BNZ tmp498
; --- source lines 2868-2896 ---
epic_serial_put_char:
    BRA epic_serial_put_char
epic_serial_put_u16:
    MOVLB 0x0
    CLRF 0x099,B
    CLRF 0x09A,B
    CALL __pa55
    CLRF 0x09F,B
    BRA epic_serial_put_u16_L3
epic_serial_put_u16_L3:
    MOVFF 0x09B, 0x0B9
    MOVFF 0x09C, 0x0BA
    MOVFF 0x09D, 0x0BB
    MOVFF 0x09E, 0x0BC
    CALL __pa44
    CALL __udiv_u32
    MOVFF 0x000, 0x0A2
    MOVFF 0x001, 0x0A3
    MOVFF 0x002, 0x0A4
    MOVFF 0x003, 0x0A5
    MOVFF 0x0A2, 0x0A6
    MOVFF 0x0A6, 0x0B9
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0BA,B
    CALL __mul_u8
    MOVFF 0x000, 0x0A8
    MOVFF 0x09B, 0x0A6
    MOVLB 0x0
; --- source lines 3141-3169 ---
    BTFSC 0x0000,0,A
    ADDLW 0x01
    MOVWF 0x0A8,B
    MOVFF 0x0A5, 0x0A9
    MOVFF 0x0A6, 0x0AA
    MOVFF 0x0A7, 0x0AB
    MOVFF 0x0A8, 0x0AC
    CLRF 0x0AD,B
    BRA epic_serial_put_idec_L17
epic_serial_put_idec_L17:
    MOVFF 0x0A9, 0x0B9
    MOVFF 0x0AA, 0x0BA
    MOVFF 0x0AB, 0x0BB
    MOVFF 0x0AC, 0x0BC
    CALL __pa44
    CALL __udiv_u32
    MOVFF 0x000, 0x09B
    MOVFF 0x001, 0x09C
    MOVFF 0x002, 0x09D
    MOVFF 0x003, 0x09E
    MOVFF 0x09B, 0x0B0
    MOVFF 0x0B0, 0x0B9
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0BA,B
    CALL __mul_u8
    MOVFF 0x000, 0x0B6
    MOVFF 0x0A9, 0x0B0
    MOVLB 0x0
; --- source lines 3261-3289 ---
    MOVF 0x0A5,W,B
    BNZ tmp315
    MOVF 0x0A6,W,B
    BNZ tmp315
    BRA epic_serial_put_idec_L.loopexit4
tmp315:
    MOVFF 0x0A5, 0x0AE
    MOVFF 0x0A6, 0x0AF
    BRA epic_serial_put_idec_L32
epic_serial_put_idec_L.preheader10:
    MOVFF 0x09F, 0x0B9
    MOVFF 0x0A0, 0x0BA
    MOVFF 0x0A1, 0x0BB
    MOVFF 0x0A2, 0x0BC
    CALL __pa44
    CALL __udiv_u32
    MOVFF 0x000, 0x0B0
    MOVFF 0x001, 0x0B1
    MOVFF 0x002, 0x0B2
    MOVFF 0x003, 0x0B3
    MOVFF 0x0B0, 0x0B4
    MOVFF 0x0B4, 0x0B9
    MOVLB 0x0
    MOVLW 0xF6
    MOVWF 0x0BA,B
    CALL __mul_u8
    MOVFF 0x000, 0x0B6
    MOVFF 0x09F, 0x0B4
    MOVLB 0x0
; --- source lines 4495-4514 ---
    MOVFF 0x045, 0x043
    BRA tmp411
tmp410:
    CLRF 0x043,A
tmp411:
    MOVF 0x043,W,A
    BZ console_rx_byte_L209
    CALL __pa70
    BRA console_rx_byte_L.loopexit26
console_rx_byte_L209:
    MOVFF 0x08E, 0x097
    MOVFF 0x08F, 0x098
    MOVLW 0x0A
    MOVWF 0x099,B
    CLRF 0x09A,B
    CALL __mul_u16
    MOVFF 0x000, 0x045
    MOVFF 0x001, 0x046
    MOVF 0x041,W,A
    ADDWF 0x045,W,A
; --- source lines 4834-4853 ---
    BRA tmp439
tmp439:
    MOVLB 0x0
    MOVLW 0x0C
    MOVWF 0x08C,B
    CLRF 0x08D,B
    BRA console_rx_byte_L.loopexit
tmp440:
    MOVFF 0x04B, 0x04F
    MOVFF 0x04C, 0x050
    BRA console_rx_byte_L.preheader38
console_rx_byte_L.loopexit:
    CALL __pa8
    MOVFF 0x08C, 0x099
    MOVFF 0x08D, 0x09A
    CALL epic_serial_write
    MOVFF 0x000, 0x04F
    MOVFF 0x001, 0x050
    MOVLB 0x2
    CLRF 0x06E,B
; --- source lines 4904-4959 ---
    MOVF 0x05A,W,A
    ADDWFC 0x0EA,F,A
    MOVLW 0x00
    MOVWF 0xFEF,A
    BRA console_rx_byte_L294
console_rx_byte_L294:
    RETURN
control_demo_init:
    CALL __pa27
    MOVLW 0x01
    MOVWF 0x062,B
    CLRF 0x063,B
    MOVLW 0x01
    MOVWF 0x060,B
    CLRF 0x061,B
    CALL USART_ComputeSPBRG
    MOVFF 0x000, 0x044
    MOVFF 0x001, 0x045
    MOVFF 0x044, 0x046
    MOVFF 0x045, 0x048
    CLRF 0x049,A
    MOVLB 0x2
    CLRF 0x002,B
    CLRF 0x003,B
    MOVLW 0x01
    MOVWF 0x004,B
    CLRF 0x005,B
    MOVLW 0x01
    MOVWF 0x006,B
    CLRF 0x007,B
    MOVLW 0x01
    MOVWF 0x008,B
    CLRF 0x009,B
    CLRF 0x00A,B
    CLRF 0x00B,B
    MOVFF 0x046, 0x20C
    MOVFF 0x048, 0x20D
    CLRF 0x00E,B
    CLRF 0x00F,B
    MOVLW LOW(epic_serial_on_tx_isr)
    MOVWF 0x010,B
    MOVLW HIGH(epic_serial_on_tx_isr)
    MOVWF 0x011,B
    MOVLW LOW(epic_serial_on_rx_isr)
    MOVWF 0x012,B
    MOVLW HIGH(epic_serial_on_rx_isr)
    MOVWF 0x013,B
    MOVLW 0x02
    MOVWF 0x054,A
    MOVLW 0x02
    MOVWF 0x055,A
    CALL EPIC_USART_Init
    MOVFF 0x000, 0x044
    MOVFF 0x001, 0x045
    MOVLW 0x0A
    MOVWF 0x05F,A
; --- source lines 5807-5830 ---
control_demo_task_control:
    CALL __pa72
    MOVFF 0x036, 0x03B
    CLRF 0x03A,A
    MOVF 0x034,W,A
    IORWF 0x03A,W,A
    MOVWF 0x036,A
    MOVF 0x035,W,A
    IORWF 0x03B,W,A
    MOVWF 0x037,A
    MOVFF 0x282, 0x034
    MOVFF 0x283, 0x035
    MOVFF 0x034, 0x05C
    MOVFF 0x035, 0x05D
    MOVFF 0x036, 0x060
    MOVLB 0x0
    MOVWF 0x061,B
    CALL epic_math_mul_s16
    MOVFF 0x000, 0x03A
    MOVFF 0x001, 0x03B
    MOVFF 0x002, 0x03C
    MOVFF 0x003, 0x03D
    MOVFF 0x292, 0x034
    MOVLW 0x01
; --- source lines 5854-5875 ---
    ADDLW 0x01
    MOVWF 0x047,A
    MOVFF 0x046, 0x03E
    MOVFF 0x047, 0x03F
    BRA control_demo_task_control_L16
control_demo_task_control_L16:
    MOVLB 0x2
    CLRF 0x090,B
    CLRF 0x091,B
    MOVFF 0x286, 0x042
    MOVFF 0x287, 0x043
    MOVFF 0x042, 0x05C
    MOVFF 0x043, 0x05D
    MOVFF 0x03E, 0x060
    MOVFF 0x03F, 0x061
    CALL epic_math_mul_s16
    MOVFF 0x000, 0x046
    MOVFF 0x001, 0x047
    MOVFF 0x002, 0x048
    MOVFF 0x003, 0x049
    MOVFF 0x288, 0x03E
    MOVFF 0x289, 0x03F
; --- source lines 6105-6126 ---
    BRA control_demo_task_control_Lepic_pid_update.exit
control_demo_task_control_L38:
    MOVFF 0x293, 0x03E
    MOVLW 0x01
    ANDWF 0x03E,F,A
    MOVF 0x03E,W,A
    BZ control_demo_task_control_L41
    BRA control_demo_task_control_L46
control_demo_task_control_L41:
    MOVFF 0x284, 0x03E
    MOVFF 0x285, 0x03F
    MOVFF 0x03E, 0x05C
    MOVFF 0x03F, 0x05D
    MOVFF 0x036, 0x060
    MOVFF 0x037, 0x061
    CALL epic_math_mul_s16
    MOVFF 0x000, 0x042
    MOVFF 0x001, 0x043
    MOVFF 0x002, 0x044
    MOVFF 0x003, 0x045
    RCALL __pa17
    MOVF 0x042,W,A
; --- source lines 6645-6663 ---
control_demo_task_eeprom_L11:
    MOVFF 0x03A, 0x2AB
    MOVLB 0x2
    CLRF 0x06B,B
    MOVFF 0x2A2, 0x034
    CLRF 0x03C,A
    MOVFF 0x034, 0x03D
    CALL EPIC_EEPROM_WriteByte
    CALL __pa52
    MOVLB 0x2
    MOVLW 0x01
    MOVWF 0x068,B
    CLRF 0x069,B
    BRA control_demo_task_eeprom_L39
control_demo_task_eeprom_L14:
    CALL EPIC_EEPROM_IsWriteComplete
    MOVFF 0x000, 0x034
    MOVF 0x034,W,A
    BNZ control_demo_task_eeprom_L17
; --- source lines 6666-6684 ---
    RCALL __pa36
    MOVLW 0x01
    MOVWF 0x06A,B
    MOVFF 0x2A3, 0x034
    MOVLW 0x01
    MOVWF 0x03C,A
    MOVFF 0x034, 0x03D
    CALL EPIC_EEPROM_WriteByte
    CALL __pa52
    MOVLB 0x2
    MOVLW 0x02
    MOVWF 0x068,B
    CLRF 0x069,B
    BRA control_demo_task_eeprom_L39
control_demo_task_eeprom_L22:
    CALL EPIC_EEPROM_IsWriteComplete
    MOVFF 0x000, 0x034
    MOVF 0x034,W,A
    BNZ control_demo_task_eeprom_L25
; --- source lines 7009-7073 ---
main:
    MOVLW 0x78
    MOVWF 0x01A,A
    CLRF 0x01B,A
    CLRF 0x01C,A
    CLRF 0x01D,A
    CALL epic_harness_init
    CALL control_demo_init
    CALL epic_taskmgr_init
    MOVLW LOW(task_stimulus)
    MOVWF 0x01A,A
    MOVLW HIGH(task_stimulus)
    CALL __pa74
    MOVLW 0x01
    MOVWF 0x01E,A
    CLRF 0x01F,A
    CLRF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(control_demo_task_control)
    MOVWF 0x01A,A
    MOVLW HIGH(control_demo_task_control)
    CALL __pa74
    MOVLW 0x05
    MOVWF 0x01E,A
    CLRF 0x01F,A
    MOVLW 0x01
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(control_demo_task_console)
    MOVWF 0x01A,A
    MOVLW HIGH(control_demo_task_console)
    CALL __pa74
    MOVLW 0x01
    MOVWF 0x01E,A
    CLRF 0x01F,A
    MOVLW 0x02
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(control_demo_task_eeprom)
    MOVWF 0x01A,A
    MOVLW HIGH(control_demo_task_eeprom)
    CALL __pa74
    MOVLW 0x05
    MOVWF 0x01E,A
    CLRF 0x01F,A
    MOVLW 0x03
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    MOVLW LOW(control_demo_task_heartbeat)
    MOVWF 0x01A,A
    MOVLW HIGH(control_demo_task_heartbeat)
    CALL __pa74
    MOVLW 0x32
    MOVWF 0x01E,A
    CLRF 0x01F,A
    MOVLW 0x04
    MOVWF 0x020,A
    CALL epic_taskmgr_spawn
    MOVFF 0x000, 0x010
    CLRF 0x01A,A
    MOVLW 0x03
; --- source lines 7322-7341 ---
    BRA main_L77
main_L74:
    CALL __pa1
    MOVLW 0x38
    CALL __pa5
    CALL epic_harness_log
    BRA main_L77
main_L77:
    CALL control_demo_eeprom_writes
    CALL __pa22
    MOVFF 0x016, 0x01A
    MOVFF 0x017, 0x01B
    MOVLW 0x0A
    MOVWF 0x01C,A
    CLRF 0x01D,A
    CALL __urem_u16
    MOVFF 0x000, 0x018
    MOVFF 0x001, 0x019
    MOVF 0x018,W,A
    BNZ main_L81
; --- source lines 8028-8046 ---
    MOVWF 0x000,A
    RETURN
epic_dispatch_all_irqs_isr:
    MOVFF 0xFF2, 0x0D2
    MOVLW 0x04
    MOVLB 0x0
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L4
    BRA epic_dispatch_all_irqs_isr_LTIMER0_IRQHandler.exit
epic_dispatch_all_irqs_isr_L4:
    MOVLW 0x04
    MOVWF 0x0D8,B
    CLRF 0x0D9,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D4
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L7
; --- source lines 8085-8103 ---
    MOVLW 0x02
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVLW 0x00
    ANDWF 0x0D3,W,B
    MOVWF 0x0D5,B
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L18
    MOVF 0x0D5,W,B
    BNZ epic_dispatch_all_irqs_isr_L18
    BRA epic_dispatch_all_irqs_isr_LTIMER2_IRQHandler.exit
epic_dispatch_all_irqs_isr_L18:
    MOVLW 0x06
    MOVWF 0x0D8,B
    CLRF 0x0D9,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D4
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L21
; --- source lines 8159-8177 ---
    MOVF 0x0D5,W,B
    BNZ epic_dispatch_all_irqs_isr_L30
    BRA epic_dispatch_all_irqs_isr_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L30:
    MOVFF 0xF9D, 0x0D4
    MOVLW 0x10
    ANDWF 0x0D4,W,B
    MOVWF 0x0D6,B
    MOVF 0x0D6,W,B
    BNZ epic_dispatch_all_irqs_isr_L34
    BRA epic_dispatch_all_irqs_isr_LUSART_TX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L34:
    MOVLW 0x0A
    MOVWF 0x0D8,B
    CLRF 0x0D9,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D4
    CLRF 0x0D6,B
    MOVF 0x0D4,W,B
; --- source lines 8228-8246 ---
    MOVLB 0x0
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVLW 0x00
    ANDWF 0x0D3,W,B
    MOVWF 0x0D5,B
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L46
    MOVF 0x0D5,W,B
    BNZ epic_dispatch_all_irqs_isr_L46
    BRA epic_dispatch_all_irqs_isr_LUSART_RX_IRQHandler.exit
epic_dispatch_all_irqs_isr_L46:
    MOVLW 0x0B
    MOVWF 0x0D8,B
    CLRF 0x0D9,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D2
    MOVF 0x0D2,W,B
    BNZ epic_dispatch_all_irqs_isr_L49
; --- source lines 8290-8308 ---
    MOVF 0x0D6,W,B
    BNZ epic_dispatch_all_irqs_isr_L60
    BRA epic_dispatch_all_irqs_isr_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_isr_L60:
    MOVFF 0xFA0, 0x0D2
    MOVLW 0x10
    ANDWF 0x0D2,W,B
    MOVWF 0x0D4,B
    MOVF 0x0D4,W,B
    BNZ epic_dispatch_all_irqs_isr_L64
    BRA epic_dispatch_all_irqs_isr_LEEPROM_IRQHandler.exit
epic_dispatch_all_irqs_isr_L64:
    MOVLW 0x0F
    MOVWF 0x0D8,B
    CLRF 0x0D9,B
    CALL EPIC_IRQ_GetFlag_isr
    MOVFF 0x000, 0x0D2
    MOVF 0x0D2,W,B
    BNZ epic_dispatch_all_irqs_isr_L67
; --- source lines 8496-8517 ---
    MOVF 0x069,W,B
    ANDLW 0x1F
    MOVWF 0x06D,B
    MOVF 0x06D,F,B
    BTFSC 0xFD8,2,A
    BRA tmp539
tmp538:
    BCF 0xFD8,0,A
    MOVLB 0x0
    RLCF 0x065,F,B
    RLCF 0x066,F,B
    RLCF 0x067,F,B
    RLCF 0x068,F,B
    DECFSZ 0x06D,F,B
    BRA tmp538
tmp539:
    MOVFF 0x065, 0x000
    MOVFF 0x066, 0x001
    MOVFF 0x067, 0x002
    MOVFF 0x068, 0x003
    RETURN
__udiv_u32:
; --- source lines 8543-8587 ---
    SUBWFB 0x0C4,F,B
    BNC tmp541
    BSF 0x0B9,0,B
    BRA tmp542
tmp541:
    MOVF 0x0BD,W,B
    ADDWF 0x0C1,F,B
    MOVF 0x0BE,W,B
    ADDWFC 0x0C2,F,B
    MOVF 0x0BF,W,B
    ADDWFC 0x0C3,F,B
    MOVF 0x0C0,W,B
    ADDWFC 0x0C4,F,B
tmp542:
    DECFSZ 0x0C5,F,B
    BRA tmp540
    MOVFF 0x0B9, 0x000
    MOVFF 0x0BA, 0x001
    MOVFF 0x0BB, 0x002
    MOVFF 0x0BC, 0x003
    RETURN
__mul_u8:
    MOVLB 0x0
    MOVF 0x0B9,W,B
    MULWF 0x0BA,B
    MOVFF 0xFF3, 0x000
    RETURN
__mul_u16:
    MOVLB 0x0
    MOVF 0x097,W,B
    MULWF 0x099,B
    MOVFF 0xFF3, 0x09B
    MOVFF 0xFF4, 0x09C
    MOVF 0x097,W,B
    MULWF 0x09A,B
    MOVF 0xFF3,W,A
    ADDWF 0x09C,F,B
    MOVF 0x098,W,B
    MULWF 0x099,B
    MOVF 0xFF3,W,A
    ADDWF 0x09C,F,B
    MOVFF 0x09B, 0x000
    MOVFF 0x09C, 0x001
    RETURN
__urem_u16:
; --- source lines 8597-8616 ---
    RLCF 0x01F,F,A
    MOVF 0x01C,W,A
    SUBWF 0x01E,F,A
    MOVF 0x01D,W,A
    SUBWFB 0x01F,F,A
    BNC tmp544
    BSF 0x01A,0,A
    BRA tmp545
tmp544:
    MOVF 0x01C,W,A
    ADDWF 0x01E,F,A
    MOVF 0x01D,W,A
    ADDWFC 0x01F,F,A
tmp545:
    DECFSZ 0x020,F,A
    BRA tmp543
    MOVFF 0x01E, 0x000
    MOVFF 0x01F, 0x001
    RETURN
__start:
; --- source lines 8998-9021 ---
    IORWF 0x049,W,A
    MOVWF 0x043,A
    MOVF 0x042,W,A
    IORWF 0x04A,W,A
    MOVWF 0x044,A
    MOVFF 0x043, 0x097
    MOVLB 0x0
    MOVWF 0x098,B
    RETURN
__pa12:
    ADDWFC 0xF7,F,A
    MOVLW 0x00
    ADDWFC 0xF8,F,A
    TBLRD*
    RETURN
__pa20:
    MOVFF 0x000, 0x034
    MOVFF 0x001, 0x035
    RETURN
__pa22:
    MOVFF 0x000, 0x016
    MOVFF 0x001, 0x017
    RETURN
__pa23:
; --- source lines 9026-9045 ---
    MOVF 0xFF4,W,A
    ADDWFC 0x0EA,F,A
    MOVLW 0x16
    MULWF 0x045,A
    MOVF 0xFF3,W,A
    ADDWF 0x0EA,F,A
    RETURN
__pa24:
    MOVLB 0x2
    MOVWF 0x014,B
    MOVLB 0x0
    MOVLW 0x0A
    MOVWF 0x0B9,B
    CLRF 0x0BA,B
    RETURN
__pa25:
    MOVFF 0x000, 0x041
    MOVFF 0x001, 0x042
    RETURN
__pa26:
; --- source lines 9126-9163 ---
    MOVLW LOW(log_fire_ticks.hx)
    MOVWF 0xF6,A
    MOVLW HIGH(log_fire_ticks.hx)
    MOVWF 0xF7,A
    MOVLW UPPER(log_fire_ticks.hx)
    MOVWF 0xF8,A
    MOVF 0x016,W,A
    ADDWF 0xF6,F,A
    MOVF 0x017,W,A
    RETURN
__pa51:
    ADDWF 0x0E9,F,A
    MOVLW 0x00
    ADDWFC 0x0EA,F,A
    RETURN
__pa52:
    MOVFF 0x000, 0x036
    MOVFF 0x001, 0x037
    RETURN
__pa55:
    MOVFF 0x097, 0x09B
    MOVFF 0x098, 0x09C
    MOVFF 0x099, 0x09D
    MOVFF 0x09A, 0x09E
    RETURN
__pa57:
    MOVWF 0x04F,A
    MOVFF 0x021, 0x04C
    MOVFF 0x022, 0x04D
    CLRF 0x051,A
    MOVLW 0x01
    SUBWF 0x04C,W,A
    RETURN
__pa66:
    MOVFF 0x000, 0x01A
    MOVFF 0x001, 0x01B
    RETURN
__pa68:
