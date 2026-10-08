    list p=p18f4550
    radix hex

; 16-bit increment carry idiom, console cluster excerpt
inc_0:
    MOVF 0x020,W,A
    ADDLW 1
    MOVWF 0x020,A
    MOVF 0x021,W,A
    BTFSC STATUS,C,A
    ADDLW 0x01
    MOVWF 0x021,A
inc_1:
    MOVF 0x030,W,B
    ADDLW 1
    MOVWF 0x030,B
    MOVF 0x031,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x031,B
inc_2:
    MOVF 0x040,W,A
    ADDLW 1
    MOVWF 0x040,A
    MOVF 0x041,W,A
    BTFSC STATUS,C,A
    ADDLW 0x01
    MOVWF 0x041,A
inc_3:
    MOVF 0x050,W,B
    ADDLW 1
    MOVWF 0x050,B
    MOVF 0x051,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x051,B
inc_4:
    MOVF 0x060,W,A
    ADDLW 1
    MOVWF 0x060,A
    MOVF 0x061,W,A
    BTFSC STATUS,C,A
    ADDLW 0x01
    MOVWF 0x061,A
inc_5:
    MOVF 0x070,W,B
    ADDLW 1
    MOVWF 0x070,B
    MOVF 0x071,W,B
    BTFSC 0xFD8,0,A
    ADDLW 0x01
    MOVWF 0x071,B
inc_6:
    MOVF 0x080,W,A
    ADDLW 1
    MOVWF 0x080,A
    MOVF 0x081,W,A
    BTFSC STATUS,C,A
    ADDLW 0x01
    MOVWF 0x081,A
negatives:
near_miss:
    MOVF 0x090,W,A
    ADDLW 2 ; plus-2 is a different idiom
    MOVWF 0x090,A
    MOVF 0x091,W,A
    BTFSC STATUS,C,A
    ADDLW 1
    MOVWF 0x091,A
banked:
    MOVF 0x092,W,A
    ADDLW 1
    MOVWF 0x092,A
    MOVF 0x093,W,A
    MOVLB 0x01 ; interleaved select breaks the strict form
    BTFSC STATUS,C,A
    ADDLW 1
    MOVWF 0x093,A
single:
    MOVF 0x094,W,A ; one byte only, no carry lane
    ADDLW 1
    MOVWF 0x094,A
