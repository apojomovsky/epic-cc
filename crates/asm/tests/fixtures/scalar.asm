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
    ANDLW 0x07
    MOVWF 0x23
    MOVF 0x23, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x24
    ; phi copies for pred main
    MOVLW 0x00
    MOVWF 0x25
    MOVLW 0x00
    MOVWF 0x26
    MOVLW 0x00
    MOVWF 0x3E
    MOVF 0x24, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L4
    GOTO main_L33
main_L4:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x25, W
    ANDLW 0x01
    MOVWF 0x27
    MOVF 0x27, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x28
    MOVF 0x28, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L20
    GOTO main_L9
main_L9:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x26, W
    ADDWF 0x25, W
    MOVWF 0x29
    MOVLW 0x02
    SUBWF 0x25, W
    MOVLW 0x00
    BTFSC STATUS, 0 ; C
    MOVLW 0x01
    BTFSC STATUS, 2 ; Z
    MOVLW 0x00
    MOVWF 0x2A
    MOVF 0x29, W
    IORLW 0x10
    MOVWF 0x2B
    MOVF 0x2A, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVF 0x2B, W
    MOVWF 0x2C
    GOTO tmp1
tmp0:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x29, W
    MOVWF 0x2C
tmp1:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x25, W
    XORLW 0x04
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2D
    MOVF 0x2C, W
    ADDLW 0x01
    MOVWF 0x2E
    MOVF 0x2D, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp2
    MOVF 0x2B, W
    MOVWF 0x2F
    GOTO tmp3
tmp2:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2E, W
    MOVWF 0x2F
tmp3:
    MOVLW 0xC8
    BCF STATUS, 5
    BCF STATUS, 6
    SUBWF 0x2F, W
    MOVLW 0x00
    BTFSC STATUS, 0 ; C
    MOVLW 0x01
    BTFSC STATUS, 2 ; Z
    MOVLW 0x00
    MOVWF 0x30
    MOVF 0x2F, W
    XORLW 0x55
    MOVWF 0x31
    MOVF 0x30, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp4
    MOVF 0x31, W
    MOVWF 0x32
    GOTO tmp5
tmp4:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2F, W
    MOVWF 0x32
tmp5:
    ; phi copies for pred main_L9
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x32, W
    MOVWF 0x3B
    GOTO main_L29
main_L20:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x25, W
    SUBWF 0x26, W
    MOVWF 0x33
    MOVF 0x33, W
    XORLW 0x55
    MOVWF 0x34
    MOVF 0x25, W
    XORLW 0x01
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x35
    MOVF 0x34, W
    IORLW 0x80
    MOVWF 0x36
    MOVF 0x35, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp6
    MOVF 0x36, W
    MOVWF 0x37
    GOTO tmp7
tmp6:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x34, W
    MOVWF 0x37
tmp7:
    MOVLW 0x0A
    BCF STATUS, 5
    BCF STATUS, 6
    SUBWF 0x37, W
    MOVLW 0x00
    BTFSS STATUS, 0 ; C
    MOVLW 0x01
    MOVWF 0x38
    MOVF 0x37, W
    ADDLW 0x03
    MOVWF 0x39
    MOVF 0x38, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp8
    MOVF 0x39, W
    MOVWF 0x3A
    GOTO tmp9
tmp8:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x37, W
    MOVWF 0x3A
tmp9:
    ; phi copies for pred main_L20
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x3A, W
    MOVWF 0x3B
    GOTO main_L29
main_L29:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x25, W
    ADDLW 0x01
    MOVWF 0x3C
    MOVF 0x3C, W
    XORWF 0x23, W
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x3D
    ; phi copies for pred main_L29
    MOVF 0x3C, W
    MOVWF 0x25
    MOVF 0x3B, W
    MOVWF 0x26
    MOVF 0x3B, W
    MOVWF 0x3E
    MOVF 0x3D, W
    BTFSC STATUS, 2 ; Z
    GOTO main_L4
    GOTO main_L33
main_L33:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x3E, W
    MOVWF 0x21
    RETURN

    end
