; pic8 -- integer spine milestone 2 (isel)
    list p=16f877a
    radix hex
STATUS equ 0x03
FSR    equ 0x04
INDF   equ 0x00
PCL    equ 0x02

    org 0x0000
    goto __start

sum:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x4C, W
    MOVWF 0x50
    MOVF 0x4E, W
    MOVWF 0x51
    MOVF 0x4F, W
    MOVWF 0x52
    MOVF 0x51, W
    MOVWF 0x53
    MOVF 0x53, W
    ADDWF 0x50, W
    MOVWF 0x54
    MOVF 0x54, W
    MOVWF 0x71
    RETURN

pick:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x4C, W
    MOVWF 0x51
    MOVF 0x51, W
    MOVWF 0x52
    CLRF 0x53
    BCF STATUS, 7
    MOVF 0x52, W
    ADDLW 0x4D
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x54
    MOVF 0x54, W
    MOVWF 0x71
    RETURN

mk:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x4C, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x4E, W
    MOVWF INDF
    MOVF 0x4C, W
    ADDLW 0x02
    MOVWF FSR
    MOVF 0x4F, W
    MOVWF INDF
    MOVF 0x4C, W
    ADDLW 0x03
    MOVWF FSR
    MOVF 0x50, W
    MOVWF INDF
    RETURN

main:
    MOVLW 0x32
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x4C
    MOVLW 0x00
    MOVWF 0x4D
    MOVLW 0x03
    MOVWF 0x4E
    MOVLW 0x34
    MOVWF 0x4F
    MOVLW 0x12
    MOVWF 0x50
    CALL mk
    MOVF 0x32, W
    MOVWF 0x20
    MOVF 0x33, W
    MOVWF 0x21
    MOVF 0x34, W
    MOVWF 0x22
    MOVF 0x35, W
    MOVWF 0x23
    MOVF 0x20, W
    MOVWF 0x4C
    MOVF 0x21, W
    MOVWF 0x4D
    MOVF 0x22, W
    MOVWF 0x4E
    MOVF 0x23, W
    MOVWF 0x4F
    CALL sum
    MOVF 0x71, W
    MOVWF 0x36
    MOVF 0x36, W
    MOVWF 0x24
    MOVLW 0x02
    MOVWF 0x26
    MOVLW 0x5A
    MOVWF 0x29
    MOVF 0x26, W
    MOVWF 0x37
    MOVF 0x37, W
    MOVWF 0x38
    CLRF 0x39
    BCF STATUS, 7
    MOVF 0x38, W
    ADDLW 0x27
    MOVWF FSR
    MOVLW 0x11
    MOVWF INDF
    MOVF 0x24, W
    MOVWF 0x3A
    MOVF 0x26, W
    MOVWF 0x4C
    MOVF 0x27, W
    MOVWF 0x4D
    MOVF 0x28, W
    MOVWF 0x4E
    MOVF 0x29, W
    MOVWF 0x4F
    MOVF 0x2A, W
    MOVWF 0x50
    CALL pick
    MOVF 0x71, W
    MOVWF 0x3B
    MOVF 0x3A, W
    ADDWF 0x3B, W
    MOVWF 0x3C
    MOVF 0x3C, W
    MOVWF 0x24
    MOVLW 0x01
    MOVWF 0x2C
    MOVLW 0x02
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
    MOVLW 0x03
    MOVWF 0x30
    MOVF 0x24, W
    MOVWF 0x3D
    MOVF 0x3D, W
    MOVWF 0x3E
    CLRF 0x3F
    MOVF 0x2C, W
    MOVWF 0x40
    MOVF 0x40, W
    MOVWF 0x41
    CLRF 0x42
    MOVF 0x3E, W
    ADDWF 0x41, W
    MOVWF 0x43
    MOVF 0x3F, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x42, W
    MOVWF 0x44
    MOVF 0x2E, W
    MOVWF 0x45
    MOVF 0x2F, W
    MOVWF 0x46
    MOVF 0x45, W
    ADDWF 0x43, W
    MOVWF 0x47
    MOVF 0x46, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x44, W
    MOVWF 0x48
    MOVF 0x30, W
    MOVWF 0x49
    MOVF 0x47, W
    MOVWF 0x4A
    MOVF 0x4A, W
    ADDWF 0x49, W
    MOVWF 0x4B
    MOVF 0x4B, W
    MOVWF 0x24
    RETURN

__start:
    CALL main
    SLEEP

    end
