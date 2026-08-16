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

    org 0x0004
isr:
    MOVWF 0x75
    SWAPF 0x75, F
    SWAPF STATUS, W
    MOVWF 0x76
    MOVF PCLATH, W
    MOVWF 0x77
    MOVF FSR, W
    MOVWF 0x78
    MOVF 0x71, W
    MOVWF 0x79
    MOVF 0x72, W
    MOVWF 0x7A
    MOVF 0x73, W
    MOVWF 0x7B
    MOVF 0x74, W
    MOVWF 0x7C
    MOVLW 0x00
    MOVWF PCLATH
    MOVLW 0x55
    MOVWF 0x06
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x2C
    MOVF 0x2C, W
    MOVWF 0x2E
    MOVLW PAGE(bump_isr)
    MOVWF PCLATH
    CALL bump_isr
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x2D
    MOVF 0x2D, W
    MOVWF 0x20
    MOVF 0x79, W
    MOVWF 0x71
    MOVF 0x7A, W
    MOVWF 0x72
    MOVF 0x7B, W
    MOVWF 0x73
    MOVF 0x7C, W
    MOVWF 0x74
    MOVF 0x77, W
    MOVWF PCLATH
    MOVF 0x78, W
    MOVWF FSR
    SWAPF 0x76, W
    MOVWF STATUS
    SWAPF 0x75, W
    RETFIE

__start:
    MOVLW PAGE(main)
    MOVWF PCLATH
    CALL main
    SLEEP

bump:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2A, W
    ADDLW 0x01
    MOVWF 0x2B
    MOVF 0x2B, W
    MOVWF 0x71
    RETURN

main:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x21, W
    MOVWF 0x22
    MOVF 0x22, W
    MOVWF 0x20
    MOVLW 0x11
    MOVWF 0x06
    MOVF 0x20, W
    MOVWF 0x23
    MOVF 0x23, W
    MOVWF 0x2A
    MOVLW PAGE(bump)
    MOVWF PCLATH
    CALL bump
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x24
    MOVF 0x24, W
    MOVWF 0x20
    MOVF 0x20, W
    MOVWF 0x25
    MOVF 0x25, W
    ADDLW 0x01
    MOVWF 0x26
    MOVF 0x26, W
    MOVWF 0x20
    MOVF 0x20, W
    MOVWF 0x27
    MOVLW 0x02
    MOVWF 0x2A
    CALL bump
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x28
    MOVF 0x27, W
    ADDWF 0x28, W
    MOVWF 0x29
    MOVF 0x29, W
    MOVWF 0x20
    MOVLW 0x22
    MOVWF 0x06
    RETURN

bump_isr:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2E, W
    ADDLW 0x01
    MOVWF 0x2F
    MOVF 0x2F, W
    MOVWF 0x71
    RETURN

    end
