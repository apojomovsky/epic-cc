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
    ADDLW 0x01
    MOVWF 0x23
    MOVF 0x23, W
    MOVWF 0x21
    RETURN

    end
