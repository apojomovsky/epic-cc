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
    MOVWF 0x31
    MOVF 0x21, W
    MOVWF 0x32
    MOVF 0x31, W
    ANDLW 0x03
    MOVWF 0x33
    MOVF 0x32, W
    ANDLW 0x00
    MOVWF 0x34
    MOVF 0x33, W
    CALL __read_table
    MOVWF 0x35
    MOVF 0x33, W
    ADDLW 0x28
    MOVWF FSR
    MOVF 0x35, W
    MOVWF INDF
    MOVF 0x33, W
    ADDLW 0x28
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x36
    MOVF 0x36, W
    MOVWF 0x30
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
