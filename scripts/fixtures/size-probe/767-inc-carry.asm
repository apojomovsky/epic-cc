; control-demo.asm @ 826e77c: 19 inc-carry sites
; each block below is verbatim from the source listing;
; block i covers source lines lo-hi (1-based) as noted.
; --- source lines 1682-1693 ---
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
; --- source lines 3498-3517 ---
    BRA console_rx_byte_L.loopexit49
tmp332:
    MOVF 0x041,W,A
    ADDLW 0x01
    MOVWF 0x041,A
    MOVF 0x042,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x042,A
    MOVFF 0x04F, 0x043
    MOVF 0x049,W,A
    ADDLW 0x01
    MOVWF 0x049,A
    MOVF 0x04A,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x04A,A
    BRA console_rx_byte_L.preheader48
console_rx_byte_L.loopexit49:
    MOVFF 0x075, 0xFE9
; --- source lines 3652-3672 ---
    BRA console_rx_byte_L.loopexit45
tmp342:
    MOVF 0x053,W,A
    ADDLW 0x01
    MOVWF 0x053,A
    MOVF 0x054,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x054,A
    MOVFF 0x041, 0x05D
    MOVLB 0x0
    MOVF 0x064,W,B
    ADDLW 0x01
    MOVWF 0x064,B
    MOVF 0x065,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x065,B
    BRA console_rx_byte_L.preheader44
console_rx_byte_L.loopexit45:
    MOVFF 0x078, 0xFE9
; --- source lines 3990-4010 ---
    BRA console_rx_byte_L.loopexit42
tmp358:
    MOVF 0x055,W,A
    ADDLW 0x01
    MOVWF 0x055,A
    MOVF 0x056,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x056,A
    MOVFF 0x045, 0x05F
    MOVLB 0x0
    MOVF 0x066,W,B
    ADDLW 0x01
    MOVWF 0x066,B
    MOVF 0x067,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x067,B
    BRA console_rx_byte_L.preheader41
console_rx_byte_L.loopexit42:
    MOVFF 0x07B, 0xFE9
; --- source lines 4114-4133 ---
tmp366:
    MOVLB 0x0
    MOVF 0x06A,W,B
    ADDLW 0x01
    MOVWF 0x06A,B
    MOVF 0x06B,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x06B,B
    MOVFF 0x045, 0x07E
    MOVF 0x07F,W,B
    ADDLW 0x01
    MOVWF 0x07F,B
    MOVF 0x080,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x080,B
    BRA console_rx_byte_L.preheader40
console_rx_byte_L.preheader36:
    MOVFF 0x081, 0xFE9
; --- source lines 4175-4194 ---
    BRA console_rx_byte_L.loopexit31
tmp373:
    MOVF 0x06C,W,B
    ADDLW 0x01
    MOVWF 0x06C,B
    MOVF 0x06D,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x06D,B
    MOVFF 0x045, 0x090
    MOVF 0x081,W,B
    ADDLW 0x01
    MOVWF 0x081,B
    MOVF 0x082,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x082,B
    BRA console_rx_byte_L.preheader36
console_rx_byte_L.preheader34:
    MOVFF 0x084, 0xFE9
; --- source lines 4232-4251 ---
tmp378:
    MOVLB 0x0
    MOVF 0x06E,W,B
    ADDLW 0x01
    MOVWF 0x06E,B
    MOVF 0x06F,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x06F,B
    MOVFF 0x045, 0x091
    MOVF 0x084,W,B
    ADDLW 0x01
    MOVWF 0x084,B
    MOVF 0x085,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x085,B
    BRA console_rx_byte_L.preheader34
console_rx_byte_L.preheader32:
    MOVFF 0x087, 0xFE9
; --- source lines 4289-4308 ---
tmp383:
    MOVLB 0x0
    MOVF 0x070,W,B
    ADDLW 0x01
    MOVWF 0x070,B
    MOVF 0x071,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x071,B
    MOVFF 0x045, 0x086
    MOVF 0x087,W,B
    ADDLW 0x01
    MOVWF 0x087,B
    MOVF 0x088,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x088,B
    BRA console_rx_byte_L.preheader32
console_rx_byte_L.preheader30:
    MOVFF 0x08A, 0xFE9
; --- source lines 4335-4354 ---
tmp388:
    MOVLB 0x0
    MOVF 0x072,W,B
    ADDLW 0x01
    MOVWF 0x072,B
    MOVF 0x073,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x073,B
    MOVFF 0x045, 0x089
    MOVF 0x08A,W,B
    ADDLW 0x01
    MOVWF 0x08A,B
    MOVF 0x08B,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x08B,B
    BRA console_rx_byte_L.preheader30
console_rx_byte_L.preheader28:
    CALL __pa69
; --- source lines 4537-4549 ---
tmp413:
    MOVF 0x049,W,A
    BZ tmp415
    MOVF 0x061,W,B
    ADDLW 0x01
    MOVWF 0x061,B
    MOVF 0x062,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x062,B
    MOVFF 0x041, 0x095
    MOVFF 0x047, 0x08E
    MOVFF 0x048, 0x08F
; --- source lines 6579-6590 ---
    BRA control_demo_task_console_L.loopexit
tmp453:
    MOVF 0x038,W,A
    ADDLW 0x01
    MOVWF 0x038,A
    MOVF 0x039,W,A
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x039,A
    MOVFF 0x03C, 0x03A
    MOVFF 0x03D, 0x03B
    BRA control_demo_task_console_L.preheader
