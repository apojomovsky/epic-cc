; control-demo.asm @ 826e77c: 17 clrf runs
; each block below is verbatim from the source listing;
; block i covers source lines lo-hi (1-based) as noted.
; --- source lines 1575-1583 ---
epic_harness_init:
    MOVLB 0x1
    MOVLW 0x78
    MOVWF 0x0B2,B
    CLRF 0x0B3,B
    CLRF 0x0B4,B
    CLRF 0x0B5,B
    MOVLW LOW(__const.epic_harness_init.h)
    MOVWF 0xF6,A
; --- source lines 1872-1879 ---
    BRA epic_taskmgr_run_once
epic_taskmgr_run:
    CLRF 0x034,A
    CLRF 0x035,A
    CLRF 0x036,A
    CLRF 0x037,A
    CALL epic_harness_running
    CALL __pa66
; --- source lines 1884-1891 ---
    BRA epic_taskmgr_run_L.loopexit4
tmp469:
    CLRF 0x01A,A
    CLRF 0x01B,A
    CLRF 0x01C,A
    CLRF 0x01D,A
    BRA epic_taskmgr_run_L.preheader
epic_taskmgr_run_L.loopexit4:
; --- source lines 4911-4971 ---
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
    MOVLB 0x0
    CLRF 0x060,B
    CALL EPIC_IRQ_DisableSrc
    MOVLB 0x2
    CLRF 0x014,B
    CLRF 0x015,B
    CLRF 0x016,B
    CLRF 0x017,B
    CLRF 0x018,B
    CLRF 0x019,B
    MOVLW LOW(__const.adc_init.adc)
    MOVWF 0xF6,A
; --- source lines 5155-5227 ---
tmp137:
    MOVFF 0xFEE, 0xFE6
    DECFSZ 0xFE8,F,A
    BRA tmp137
    MOVLW 0xEE
    MOVWF 0x0F6,B
    MOVLW 0x00
    MOVWF 0x0F7,B
    MOVFF 0x01E, 0x044
    MOVFF 0x044, 0xFCB
    MOVFF 0x01C, 0x044
    MOVFF 0x01D, 0x045
    MOVFF 0x044, 0x046
    MOVFF 0x045, 0x047
    BCF 0xFD8,0,A
    RLCF 0x046,F,A
    RLCF 0x047,F,A
    BCF 0xFD8,0,A
    RLCF 0x046,F,A
    RLCF 0x047,F,A
    BCF 0xFD8,0,A
    RLCF 0x046,F,A
    RLCF 0x047,F,A
    MOVLW 0x78
    ANDWF 0x046,W,A
    MOVWF 0x044,A
    MOVLW 0x00
    ANDWF 0x047,W,A
    MOVWF 0x045,A
    MOVFF 0x01A, 0x046
    MOVFF 0x01B, 0x047
    MOVLW 0x03
    ANDWF 0x046,W,A
    MOVWF 0x048,A
    MOVLW 0x00
    ANDWF 0x047,W,A
    MOVWF 0x049,A
    MOVF 0x048,W,A
    IORWF 0x044,W,A
    MOVWF 0x046,A
    MOVF 0x049,W,A
    IORWF 0x045,W,A
    MOVWF 0x047,A
    MOVLW 0x04
    IORWF 0x046,W,A
    MOVWF 0x044,A
    MOVFF 0x044, 0xFCA
    MOVLW 0x01
    MOVWF 0x021,A
    CLRF 0x022,A
    MOVLW 0x0C
    MOVWF 0x023,A
    CLRF 0x024,A
    CLRF 0x025,A
    CLRF 0x026,A
    SETF 0x027,A
    CLRF 0x028,A
    CLRF 0x029,A
    CLRF 0x02A,A
    CLRF 0x02B,A
    CLRF 0x02C,A
    CLRF 0x02D,A
    CLRF 0x02E,A
    CLRF 0x02F,A
    CLRF 0x030,A
    CLRF 0x031,A
    CLRF 0x032,A
    CLRF 0x033,A
    CLRF 0x034,A
    CLRF 0x035,A
    CLRF 0x036,A
    CALL __pa42
    MOVLW 0xFF
; --- source lines 5616-5631 ---
control_demo_init_LEPIC_CCP_Init.exit:
    MOVLW 0x7A
    MOVLB 0x2
    MOVWF 0x070,B
    MOVLW 0x02
    MOVWF 0x071,B
    MOVLW 0x04
    MOVWF 0x072,B
    CLRF 0x073,B
    CLRF 0x074,B
    CLRF 0x075,B
    CLRF 0x076,B
    CLRF 0x077,B
    CLRF 0x078,B
    CLRF 0x046,A
    CLRF 0x047,A
; --- source lines 5654-5660 ---
    BRA tmp169
tmp169:
    CLRF 0x048,A
    CLRF 0x049,A
    CLRF 0x04A,A
    BRA control_demo_init_L.preheader
tmp170:
; --- source lines 5717-5733 ---
control_demo_init_L214:
    MOVLB 0x2
    MOVLW 0xC7
    MOVWF 0x0A2,B
    CLRF 0x0A3,B
    MOVLW 0x08
    MOVWF 0x0A4,B
    MOVLW 0x80
    MOVWF 0x0A5,B
    CLRF 0x0A6,B
    MOVLW 0x08
    MOVWF 0x0A7,B
    CLRF 0x0A8,B
    CLRF 0x0A9,B
    CLRF 0x0AA,B
    MOVLW 0x01
    MOVWF 0x06C,B
; --- source lines 5743-5807 ---
control_demo_init_L218:
    MOVFF 0x2A5, 0x044
    CLRF 0x045,A
    MOVFF 0x2A6, 0x046
    CLRF 0x047,A
    MOVFF 0x046, 0x049
    CLRF 0x048,A
    MOVF 0x044,W,A
    IORWF 0x048,W,A
    MOVWF 0x046,A
    MOVF 0x045,W,A
    IORWF 0x049,W,A
    MOVWF 0x047,A
    MOVFF 0x2A7, 0x044
    CLRF 0x045,A
    MOVFF 0x2A8, 0x048
    CLRF 0x049,A
    MOVFF 0x048, 0x04B
    CLRF 0x04A,A
    MOVF 0x044,W,A
    IORWF 0x04A,W,A
    MOVWF 0x048,A
    MOVF 0x045,W,A
    IORWF 0x04B,W,A
    MOVWF 0x049,A
    MOVFF 0x2A9, 0x044
    CLRF 0x045,A
    MOVFF 0x2AA, 0x04A
    CLRF 0x04B,A
    MOVFF 0x04A, 0x04D
    CLRF 0x04C,A
    MOVF 0x044,W,A
    IORWF 0x04C,W,A
    MOVWF 0x04A,A
    MOVF 0x045,W,A
    IORWF 0x04D,W,A
    MOVWF 0x04B,A
    MOVFF 0x046, 0x282
    MOVFF 0x047, 0x283
    MOVFF 0x048, 0x284
    MOVFF 0x049, 0x285
    MOVFF 0x04A, 0x286
    MOVLB 0x2
    MOVWF 0x087,B
    CLRF 0x088,B
    CLRF 0x089,B
    MOVLW 0xE8
    MOVWF 0x08A,B
    MOVLW 0x03
    MOVWF 0x08B,B
    CLRF 0x08C,B
    CLRF 0x08D,B
    CLRF 0x08E,B
    CLRF 0x08F,B
    CLRF 0x090,B
    CLRF 0x091,B
    CLRF 0x092,B
    CLRF 0x093,B
    MOVLW 0x01
    MOVWF 0x094,B
    CLRF 0x095,B
    CLRF 0x096,B
    CLRF 0x097,B
    RETURN
control_demo_task_control:
; --- source lines 6615-6623 ---
control_demo_task_eeprom_L3:
    MOVFF 0x26C, 0x038
    MOVF 0x038,W,A
    BZ control_demo_task_eeprom_L39
    CLRF 0x034,A
    CLRF 0x035,A
    CLRF 0x036,A
    BRA control_demo_task_eeprom_L.preheader
control_demo_task_eeprom_L.preheader:
; --- source lines 7009-7016 ---
main:
    MOVLW 0x78
    MOVWF 0x01A,A
    CLRF 0x01B,A
    CLRF 0x01C,A
    CLRF 0x01D,A
    CALL epic_harness_init
    CALL control_demo_init
; --- source lines 8517-8524 ---
__udiv_u32:
    MOVLB 0x0
    CLRF 0x0C1,B
    CLRF 0x0C2,B
    CLRF 0x0C3,B
    CLRF 0x0C4,B
    MOVLW 0x20
    MOVWF 0x0C5,B
; --- source lines 9049-9066 ---
__pa27:
    CLRF 0x054,A
    MOVLW 0x6C
    MOVWF 0x055,A
    MOVLW 0xDC
    MOVWF 0x056,A
    MOVLW 0x02
    MOVWF 0x057,A
    MOVLW 0x80
    MOVWF 0x058,A
    MOVLW 0x25
    MOVWF 0x059,A
    CLRF 0x05A,A
    CLRF 0x05B,A
    CLRF 0x05C,A
    CLRF 0x05D,A
    MOVLB 0x0
    RETURN
; --- source lines 9117-9125 ---
__pa44:
    MOVLB 0x0
    MOVLW 0x0A
    MOVWF 0x0BD,B
    CLRF 0x0BE,B
    CLRF 0x0BF,B
    CLRF 0x0C0,B
    RETURN
__pa50:
