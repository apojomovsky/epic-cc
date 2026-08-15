; pic8 -- integer spine milestone 2 (isel)
    list p=16f877a
    radix hex
STATUS equ 0x03
FSR    equ 0x04
INDF   equ 0x00
PCL    equ 0x02

    org 0x0000
    goto __start

main:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x2B
    MOVF 0x21, W
    MOVWF 0x2C
    MOVF 0x2B, W
    ANDLW 0x03
    MOVWF 0x2D
    MOVF 0x2C, W
    ANDLW 0x00
    MOVWF 0x2E
    MOVF 0x2D, W
    CALL __read_table
    MOVWF 0x2F
    MOVF 0x2D, W
    ADDLW 0x22
    MOVWF FSR
    MOVF 0x2F, W
    MOVWF INDF
    MOVF 0x2D, W
    ADDLW 0x22
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x30
    MOVF 0x30, W
    MOVWF 0x2A
    RETURN

__read_table:
    ADDLW LOW(table)
    MOVWF PCL
table:
    RETLW 0x0A
    RETLW 0x14
    RETLW 0x1E
    RETLW 0x28

__start:
    CALL main
    SLEEP

    end
