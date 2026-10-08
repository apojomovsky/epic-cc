; menu-demo.asm @ 826e77c: 43 w-sfr-pair sites
; each block below is verbatim from the source listing;
; block i covers source lines lo-hi (1-based) as noted.
; --- source lines 338-346 ---
EPIC_TIMER2_Init:
    MOVFF 0xFCA, 0x097
    MOVLW 0xFB
    MOVLB 0x0
    ANDWF 0x097,W,B
    MOVWF 0x099,B
    MOVFF 0x099, 0xFCA
    MOVLW 0x06
    MOVWF 0x0A0,B
; --- source lines 400-449 ---
EPIC_TIMER2_Start:
    CALL __pa11
    MOVLW 0x04
    CALL __pa31
    MOVFF 0xFEF, 0x097
    MOVFF 0x097, 0xFCB
    CALL __pa11
    MOVLW 0x02
    CALL __pa31
    MOVFF 0xFEE, 0x097
    MOVFF 0xFEF, 0x098
    MOVFF 0x097, 0x099
    MOVFF 0x098, 0x09A
    BCF 0xFD8,0,A
    MOVLB 0x0
    RLCF 0x099,F,B
    RLCF 0x09A,F,B
    BCF 0xFD8,0,A
    RLCF 0x099,F,B
    RLCF 0x09A,F,B
    BCF 0xFD8,0,A
    RLCF 0x099,F,B
    RLCF 0x09A,F,B
    MOVLW 0x78
    ANDWF 0x099,W,B
    MOVWF 0x097,B
    MOVLW 0x00
    ANDWF 0x09A,W,B
    MOVWF 0x098,B
    CALL __pa11
    MOVFF 0xFEE, 0x099
    MOVFF 0xFEF, 0x09A
    MOVLW 0x03
    ANDWF 0x099,W,B
    MOVWF 0x095,B
    MOVLW 0x00
    ANDWF 0x09A,W,B
    MOVWF 0x096,B
    MOVF 0x095,W,B
    IORWF 0x097,W,B
    MOVWF 0x099,B
    MOVF 0x096,W,B
    IORWF 0x098,W,B
    MOVWF 0x09A,B
    MOVLW 0x04
    IORWF 0x099,W,B
    MOVWF 0x095,B
    MOVFF 0x095, 0xFCA
    GOTO __pa67
EPIC_TIMER2_Stop:
; --- source lines 694-700 ---
tmp71:
    MOVF 0x09C,W,B
    IORWF 0x099,W,B
    MOVWF 0x09B,B
    MOVFF 0x09B, 0xFB8
    CALL __pa11
    MOVFF 0xFEE, 0x099
; --- source lines 1083-1088 ---
__pa88:
    ANDWF 0x020,W,A
    MOVWF 0x022,A
    MOVFF 0x022, 0xFF2
    RETURN
EPIC_IRQ_Restore:
; --- source lines 1093-1106 ---
EPIC_IRQ_Restore_L3:
    MOVFF 0xFD0, 0x095
    MOVLW 0x80
    MOVLB 0x0
    IORWF 0x095,W,B
    MOVWF 0x096,B
    MOVFF 0x096, 0xFD0
    MOVFF 0xFF2, 0x095
    MOVLW 0x80
    IORWF 0x095,W,B
    MOVWF 0x096,B
    MOVFF 0x096, 0xFF2
    MOVFF 0xFF2, 0x095
    MOVLW 0x40
; --- source lines 1111-1119 ---
EPIC_IRQ_Restore_L10:
    MOVFF 0xFF2, 0x096
    MOVLW 0x7F
    MOVLB 0x0
    ANDWF 0x096,W,B
    MOVWF 0x097,B
    MOVFF 0x097, 0xFF2
    MOVFF 0xFF2, 0x096
    MOVLW 0xBF
; --- source lines 1214-1220 ---
__pa14:
    MOVLB 0x0
    IORWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xF9D
    RETURN
__pa72:
; --- source lines 1255-1263 ---
EPIC_IRQ_DisableSrc_L2:
    MOVFF 0xFF2, 0x0A0
    MOVLW 0xDF
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xFF2
    BRA EPIC_IRQ_DisableSrc_L32
EPIC_IRQ_DisableSrc_L5:
; --- source lines 1312-1324 ---
__pa15:
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xF9D
    RETURN
__pa74:
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xFA0
    RETURN
EPIC_IRQ_ClearFlag:
; --- source lines 1356-1364 ---
EPIC_IRQ_ClearFlag_L2:
    MOVFF 0xFF2, 0x0A0
    MOVLW 0xFB
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xFF2
    BRA EPIC_IRQ_ClearFlag_L38
EPIC_IRQ_ClearFlag_L5:
; --- source lines 1423-1435 ---
__pa16:
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xF9E
    RETURN
__pa33:
    MOVLB 0x0
    ANDWF 0x0A0,W,B
    MOVWF 0x0A2,B
    MOVFF 0x0A2, 0xFA1
    RETURN
EPIC_IRQ_GetFlag:
; --- source lines 2692-2701 ---
tmp144:
    MOVFF 0xFD5, 0x01A
    MOVLW 0x80
    ANDWF 0x01A,W,A
    MOVWF 0x01C,A
    IORWF 0x02A,W,A
    MOVWF 0x01A,A
    MOVFF 0x01A, 0xFD5
    MOVFF 0xFD5, 0x01A
    MOVLW 0x80
; --- source lines 2713-2718 ---
    RETURN
__pa78:
    MOVWF 0x01C,A
    MOVFF 0x01C, 0xFD5
    RETURN
epic_tick_on_overflow:
; --- source lines 3422-3503 ---
menu_demo_init:
    MOVLB 0x0
    MOVLW 0x10
    MOVWF 0x065,B
    MOVLW 0x02
    MOVWF 0x066,B
    CLRF 0x067,B
    CLRF 0x068,B
    CLRF 0x069,B
    CLRF 0x06A,B
    CALL __pa34
    MOVLW 0x01
    MOVWF 0x0A1,B
    CLRF 0x0A2,B
    MOVLW 0x01
    MOVWF 0x0A3,B
    CLRF 0x0A4,B
    CALL USART_ComputeSPBRG
    CALL __pa43
    MOVFF 0x01A, 0x01E
    MOVFF 0x01B, 0x024
    CLRF 0x025,A
    MOVLB 0x1
    CLRF 0x0DC,B
    CLRF 0x0DD,B
    MOVLW 0x01
    MOVWF 0x0DE,B
    CLRF 0x0DF,B
    MOVLW 0x01
    MOVWF 0x0E0,B
    CLRF 0x0E1,B
    MOVLW 0x01
    MOVWF 0x0E2,B
    CLRF 0x0E3,B
    CLRF 0x0E4,B
    CLRF 0x0E5,B
    MOVFF 0x01E, 0x1E6
    MOVFF 0x024, 0x1E7
    CLRF 0x0E8,B
    CLRF 0x0E9,B
    MOVLW LOW(epic_serial_on_tx_isr)
    MOVWF 0x0EA,B
    MOVLW HIGH(epic_serial_on_tx_isr)
    MOVWF 0x0EB,B
    MOVLW LOW(epic_serial_on_rx_isr)
    MOVWF 0x0EC,B
    MOVLW HIGH(epic_serial_on_rx_isr)
    MOVWF 0x0ED,B
    MOVLB 0x0
    MOVLW 0xDC
    MOVWF 0x095,B
    MOVLW 0x01
    MOVWF 0x096,B
    CALL EPIC_USART_Init
    CALL __pa43
    CALL __pa50
    CALL EPIC_IRQ_DisableSrc
    MOVLB 0x1
    CLRF 0x0EE,B
    CLRF 0x0EF,B
    CLRF 0x0F0,B
    CLRF 0x0F1,B
    CLRF 0x0F2,B
    CLRF 0x0F3,B
    MOVLB 0x0
    MOVLW 0x01
    MOVWF 0x095,B
    CLRF 0x096,B
    MOVLW 0x70
    MOVWF 0x097,B
    CLRF 0x098,B
    MOVLW 0x01
    MOVWF 0x099,B
    CLRF 0x09A,B
    CALL EPIC_GPIO_Init
    MOVFF 0xFF1, 0x01A
    MOVLW 0x7F
    ANDWF 0x01A,W,A
    MOVWF 0x01E,A
    MOVFF 0x01E, 0xFF1
    SETF 0x01A,A
    SETF 0x01B,A
; --- source lines 4067-4092 ---
tmp237:
    MOVFF 0xFEE, 0xFE6
    DECFSZ 0xFE8,F,A
    BRA tmp237
    MOVLW 0x60
    MOVLB 0x1
    MOVWF 0x06E,B
    MOVLW 0x01
    MOVWF 0x06F,B
    MOVLW 0x01
    MOVWF 0x0C2,A
    MOVFF 0x073, 0x022
    MOVFF 0x074, 0x023
    MOVFF 0x075, 0x028
    MOVLW 0x0F
    ANDWF 0x028,W,A
    MOVWF 0x038,A
    MOVLW 0x30
    ANDWF 0x022,W,A
    MOVWF 0x028,A
    MOVF 0x038,W,A
    IORWF 0x028,W,A
    MOVWF 0x022,A
    MOVFF 0x022, 0xFC1
    MOVFF 0x071, 0x022
    MOVFF 0x072, 0x023
; --- source lines 4585-4621 ---
tmp274:
    MOVF 0x01E,W,A
    IORWF 0x024,W,A
    MOVWF 0x01A,A
    MOVFF 0x01A, 0xFB7
    MOVFF 0x08D, 0x01A
    MOVFF 0x08E, 0x01B
    MOVFF 0x08F, 0x01E
    MOVFF 0x090, 0x01F
    MOVFF 0x01E, 0x024
    BCF 0xFD8,0,A
    RLCF 0x024,F,A
    BCF 0xFD8,0,A
    RLCF 0x024,F,A
    MOVLW 0x0C
    ANDWF 0x024,W,A
    MOVWF 0x01E,A
    MOVFF 0x091, 0x024
    MOVFF 0x092, 0x025
    MOVLW 0x03
    ANDWF 0x024,W,A
    MOVWF 0x036,A
    MOVFF 0x01A, 0x024
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
; --- source lines 5821-5829 ---
menu_demo_task_heartbeat:
    MOVFF 0x264, 0x036
    MOVFF 0xFBD, 0x034
    MOVLW 0xCF
    ANDWF 0x034,W,A
    MOVWF 0x038,A
    MOVFF 0x038, 0xFBD
    MOVFF 0x036, 0x0BF
    MOVLB 0x0
; --- source lines 6729-6809 ---
EPIC_IRQ_DisableSrc_isr_L2:
    MOVFF 0xFF2, 0x0DB
    MOVLW 0xDF
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xFF2
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L5:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xFE
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L8:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xFD
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L11:
    MOVFF 0xFA0, 0x0DB
    MOVLW 0xFD
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xFA0
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L14:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xFB
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L17:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xF7
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L20:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xEF
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L23:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xDF
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L26:
    MOVFF 0xF9D, 0x0DB
    MOVLW 0xBF
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xF9D
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_L29:
    MOVFF 0xFA0, 0x0DB
    MOVLW 0xFE
    MOVLB 0x0
    ANDWF 0x0DB,W,B
    MOVWF 0x0DD,B
    MOVFF 0x0DD, 0xFA0
    BRA EPIC_IRQ_DisableSrc_isr_L32
EPIC_IRQ_DisableSrc_isr_Ldefault.unreachable:
; --- source lines 6848-6944 ---
EPIC_IRQ_ClearFlag_isr_L2:
    MOVFF 0xFF2, 0x0D7
    MOVLW 0xFB
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xFF2
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L5:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xFE
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L8:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xFD
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L11:
    MOVFF 0xFA1, 0x0D7
    MOVLW 0xFD
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xFA1
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L14:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xFB
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L17:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xF7
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L20:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xEF
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L23:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xDF
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L26:
    MOVFF 0xF9E, 0x0D7
    MOVLW 0xBF
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xF9E
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L29:
    MOVFF 0xFA1, 0x0D7
    MOVLW 0xFE
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xFA1
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L32:
    MOVFF 0xFA1, 0x0D7
    MOVLW 0xBF
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xFA1
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_L35:
    MOVFF 0xFA1, 0x0D7
    MOVLW 0xEF
    MOVLB 0x0
    ANDWF 0x0D7,W,B
    MOVWF 0x0D9,B
    MOVFF 0x0D9, 0xFA1
    BRA EPIC_IRQ_ClearFlag_isr_L38
EPIC_IRQ_ClearFlag_isr_Ldefault.unreachable:
