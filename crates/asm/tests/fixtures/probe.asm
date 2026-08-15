; pic8 -- integer spine milestone 2 (isel)
    list p=16f877a
    radix hex
STATUS equ 0x03

    org 0x0000
    goto __start

main:
    MOVF 0x20, W
    MOVWF 0x79
    MOVF 0x79, W
    MOVWF 0x7A
    CLRF 0x7B
    MOVF 0x79, W
    XORLW 0x00
    MOVWF 0x22
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x7C
    ; phi copies for pred main
    MOVLW 0x00
    MOVWF 0x74
    MOVLW 0x00
    MOVWF 0x75
    MOVLW 0x00
    MOVWF 0x76
    MOVLW 0x00
    MOVWF 0x77
    MOVLW 0x00
    MOVWF 0x78
    MOVF 0x7C, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L8
    GOTO main_L6
main_L4:
    MOVF 0x28, W
    MOVWF 0x7D
    ; phi copies for pred main_L4
    MOVF 0x7D, W
    MOVWF 0x74
    GOTO main_L6
main_L6:
    MOVF 0x74, W
    MOVWF 0x21
    RETURN
main_L8:
    MOVF 0x75, W
    ANDLW 0x01
    MOVWF 0x7E
    MOVF 0x76, W
    ANDLW 0x00
    MOVWF 0x7F
    MOVF 0x7E, W
    XORLW 0x00
    MOVWF 0x22
    MOVF 0x7F, W
    XORLW 0x00
    IORWF 0x22, W
    MOVWF 0x22
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x25
    MOVF 0x25, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVLW 0x64
    MOVWF 0x26
    MOVLW 0x00
    MOVWF 0x27
    GOTO tmp1
tmp0:
    MOVF 0x75, W
    MOVWF 0x26
    MOVF 0x76, W
    MOVWF 0x27
tmp1:
    MOVF 0x77, W
    MOVWF 0x70
    MOVF 0x78, W
    MOVWF 0x71
    MOVF 0x26, W
    MOVWF 0x72
    MOVF 0x27, W
    MOVWF 0x73
    CALL add
    MOVF 0x23, W
    MOVWF 0x28
    MOVF 0x24, W
    MOVWF 0x29
    MOVF 0x75, W
    ADDLW 0x01
    MOVWF 0x2A
    MOVF 0x76, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDLW 0x00
    MOVWF 0x2B
    MOVF 0x2A, W
    XORWF 0x7A, W
    MOVWF 0x22
    MOVF 0x2B, W
    XORWF 0x7B, W
    IORWF 0x22, W
    MOVWF 0x22
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2C
    ; phi copies for pred main_L8
    MOVF 0x2A, W
    MOVWF 0x75
    MOVF 0x2B, W
    MOVWF 0x76
    MOVF 0x28, W
    MOVWF 0x77
    MOVF 0x29, W
    MOVWF 0x78
    MOVF 0x2C, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L8
    GOTO main_L4

add:
    MOVF 0x70, W
    ADDWF 0x72, W
    MOVWF 0x2D
    MOVF 0x71, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x73, W
    MOVWF 0x2E
    MOVF 0x2D, W
    MOVWF 0x23
    MOVF 0x2E, W
    MOVWF 0x24
    RETURN

__start:
    CALL main
    SLEEP

    end