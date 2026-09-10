; PIC baseline P1 probe (DS41236E Table 8-2): every instruction the asm
; encoder must produce byte-for-byte, cross-checked against
; `gpasm -p p12f509` 1.5.2 (2026-09-09). Covers all 33 baseline mnemonics,
; the d = 1 destination default (gpasm Message[305]), the 5-bit f field,
; the 3-bit b field, the 9-bit GOTO literal, and the TRIS/OPTION control
; ops. Register names match p12f509.inc.
    list p=12f509
    radix hex
INDF   equ 0x000
STATUS equ 0x003
FSR    equ 0x004
    org 0
    NOP
    CLRW
    OPTION
    SLEEP
    CLRWDT
    TRIS 6
    TRIS 7
    MOVWF 0x13
    CLRF 0x14
    MOVF 0x10
    MOVF 0x10, W
    MOVF 0x10, F
    ADDWF 0x11
    ADDWF 0x11, W
    ADDWF 0x11, F
    SUBWF 0x15, F
    SUBWF 0x16, W
    ANDWF 0x17, F
    IORWF 0x18, F
    XORWF 0x19, F
    COMF 0x1A, F
    DECF 0x1B, F
    DECFSZ 0x1C, F
    INCF 0x1D, F
    INCFSZ 0x1E, F
    RLF 0x1F, F
    RRF 0x10, F
    SWAPF 0x11, F
    MOVLW 0x55
    ANDLW 0x0F
    IORLW 0xF0
    XORLW 0xFF
    RETLW 0x12
    GOTO 0x1FF
    CALL 0x0FF
    BCF 0x10, 5
    BSF 0x10, 5
    BTFSC 0x10, 5
    BTFSS 0x10, 5
    BCF STATUS, 5
    BSF FSR, 5
    MOVWF INDF
    end
