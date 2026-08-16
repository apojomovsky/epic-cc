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
    MOVWF 0x2B
    MOVF 0x21, W
    MOVWF 0x2C
    MOVF 0x2B, W
    ANDLW 0x03
    MOVWF 0x2D
    MOVF 0x2C, W
    ANDLW 0x00
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF PCLATH
    MOVF 0x2D, W
    CALL __read_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2F
    BCF STATUS, 7
    MOVF 0x2D, W
    ADDLW 0x22
    MOVWF FSR
    MOVF 0x2F, W
    MOVWF INDF
    BCF STATUS, 7
    MOVF 0x2D, W
    ADDLW 0x22
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x30
    MOVF 0x30, W
    MOVWF 0x2A
    RETURN

__read_table:
    MOVWF 0x70
    MOVLW HIGH(table)
    MOVWF PCLATH
    MOVF 0x70, W
    ADDLW LOW(table)
    MOVWF PCL
    .table table 4
table:
    RETLW 0x0A
    RETLW 0x14
    RETLW 0x1E
    RETLW 0x28

    end
