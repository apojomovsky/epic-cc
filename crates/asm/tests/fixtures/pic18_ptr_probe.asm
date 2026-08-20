; pic8 -- P2 integer spine (isel-pic18)
    list p=p18f4550
    radix hex

    org 0x0000
    goto __start

main:
    MOVFF 0x004, 0x00F
    MOVFF 0x005, 0x010
    MOVLW 0x03
    ANDWF 0x00F,W,A
    MOVWF 0x011,A
    MOVLW 0x00
    ANDWF 0x010,W,A
    MOVWF 0x012,A
    MOVLW LOW(table)
    MOVWF 0xF6,A
    MOVLW HIGH(table)
    MOVWF 0xF7,A
    MOVLW UPPER(table)
    MOVWF 0xF8,A
    MOVF 0x011,W,A
    ADDWF 0xF6,F,A
    MOVLW 0x00
    ADDWFC 0xF7,F,A
    MOVLW 0x00
    ADDWFC 0xF8,F,A
    MOVF 0x012,W,A
    ADDWF 0xF7,F,A
    MOVLW 0x00
    ADDWFC 0xF8,F,A
    TBLRD*
    MOVFF 0xFF5, 0x013
    LFSR 0, 0x006
    MOVF 0x011,W,A
    ADDWF 0x0E9,F,A
    MOVLW 0x00
    ADDWFC 0x0EA,F,A
    MOVF 0x013,W,A
    MOVWF 0xFEF,A
    LFSR 0, 0x006
    MOVF 0x011,W,A
    ADDWF 0x0E9,F,A
    MOVLW 0x00
    ADDWFC 0x0EA,F,A
    MOVFF 0xFEF, 0x014
    MOVFF 0x014, 0x00E
    RETURN
__start:
    call main
    sleep
table:
    db 0x0A, 0x14, 0x1E, 0x28

    end
