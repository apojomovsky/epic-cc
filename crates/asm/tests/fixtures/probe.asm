; pic8 -- integer spine milestone 2 (isel)
    list p=16f877a
    radix hex
STATUS equ 0x03
FSR    equ 0x04
INDF   equ 0x00
PCL    equ 0x02
PCLATH equ 0x0A

    org 0x0000
    goto __start

__start:
    MOVLW 0x00
    MOVWF PCLATH
    CALL main
    SLEEP

main:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x22
    MOVF 0x22, W
    MOVWF 0x23
    CLRF 0x24
    MOVF 0x22, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x25
    ; phi copies for pred main
    MOVLW 0x00
    MOVWF 0x27
    MOVLW 0x00
    MOVWF 0x28
    MOVLW 0x00
    MOVWF 0x29
    MOVLW 0x00
    MOVWF 0x2A
    MOVLW 0x00
    MOVWF 0x2B
    MOVF 0x25, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L8
    GOTO main_L6
main_L4:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x31, W
    MOVWF 0x26
    ; phi copies for pred main_L4
    MOVF 0x26, W
    MOVWF 0x27
    GOTO main_L6
main_L6:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x27, W
    MOVWF 0x21
    RETURN
main_L8:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x28, W
    ANDLW 0x01
    MOVWF 0x2C
    MOVF 0x29, W
    ANDLW 0x00
    MOVWF 0x2D
    MOVF 0x2C, W
    XORLW 0x00
    MOVWF 0x70
    MOVF 0x2D, W
    XORLW 0x00
    IORWF 0x70, W
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2E
    MOVF 0x2E, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVLW 0x64
    MOVWF 0x2F
    MOVLW 0x00
    MOVWF 0x30
    GOTO tmp1
tmp0:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x28, W
    MOVWF 0x2F
    MOVF 0x29, W
    MOVWF 0x30
tmp1:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2A, W
    MOVWF 0x36
    MOVF 0x2B, W
    MOVWF 0x37
    MOVF 0x2F, W
    MOVWF 0x38
    MOVF 0x30, W
    MOVWF 0x39
    MOVLW 0x00
    MOVWF PCLATH
    CALL add
    MOVLW 0x00
    MOVWF PCLATH
    MOVF 0x71, W
    MOVWF 0x31
    MOVF 0x72, W
    MOVWF 0x32
    MOVF 0x28, W
    ADDLW 0x01
    MOVWF 0x33
    MOVF 0x29, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDLW 0x00
    MOVWF 0x34
    MOVF 0x33, W
    XORWF 0x23, W
    MOVWF 0x70
    MOVF 0x34, W
    XORWF 0x24, W
    IORWF 0x70, W
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x35
    ; phi copies for pred main_L8
    MOVF 0x33, W
    MOVWF 0x28
    MOVF 0x34, W
    MOVWF 0x29
    MOVF 0x31, W
    MOVWF 0x2A
    MOVF 0x32, W
    MOVWF 0x2B
    MOVF 0x35, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L8
    GOTO main_L4

add:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x36, W
    ADDWF 0x38, W
    MOVWF 0x3A
    MOVF 0x37, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x39, W
    MOVWF 0x3B
    MOVF 0x3A, W
    MOVWF 0x71
    MOVF 0x3B, W
    MOVWF 0x72
    RETURN

    end
