; pic8 -- integer spine milestone 2 (isel)
    list p=p16f877a
    radix hex
STATUS equ 0x03
FSR    equ 0x04
INDF   equ 0x00
PCL    equ 0x02
PCLATH equ 0x0A
INTCON equ 0x0B

    org 0x0000
    goto __start

__start:
    MOVLW PAGE(main)
    MOVWF PCLATH
    CALL main
    SLEEP

EPIC_IRQ_GetFlag:
    MOVF 0x20, W
    MOVWF 0x25
    MOVF 0x25, W
    ANDLW 0x07
    MOVWF 0x26
    MOVF 0x26, W
    MOVWF 0x27
    CLRF 0x28
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2A
    MOVF 0x29, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2B
    MOVF 0x2B, W
    BTFSS STATUS, 2 ; Z
    GOTO EPIC_IRQ_GetFlag_L9
    ; phi copies for pred EPIC_IRQ_GetFlag
    MOVLW 0x0B
    MOVWF 0x30
    MOVLW 0x00
    MOVWF 0x31
    GOTO EPIC_IRQ_GetFlag_L14
EPIC_IRQ_GetFlag_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2C
    MOVF 0x2C, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2D
    MOVF 0x2D, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVLW 0x0C
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
    GOTO tmp1
tmp0:
    MOVLW 0x0D
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
tmp1:
    ; phi copies for pred EPIC_IRQ_GetFlag_L9
    MOVF 0x2E, W
    MOVWF 0x30
    MOVF 0x2F, W
    MOVWF 0x31
    GOTO EPIC_IRQ_GetFlag_L14
EPIC_IRQ_GetFlag_L14:
    BTFSC 0x31, 0
    BSF STATUS, 7
    BTFSS 0x31, 0
    BCF STATUS, 7
    MOVF 0x30, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x32
    MOVF 0x2A, W
    ANDWF 0x32, W
    MOVWF 0x33
    MOVF 0x33, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x34
    MOVF 0x34, W
    MOVWF 0x35
    MOVF 0x35, W
    MOVWF 0x71
    RETURN

EPIC_IRQ_ClearFlag:
    MOVF 0x20, W
    MOVWF 0x25
    MOVF 0x25, W
    ANDLW 0x07
    MOVWF 0x26
    MOVF 0x26, W
    MOVWF 0x27
    CLRF 0x28
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2A
    MOVF 0x29, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2B
    MOVF 0x2B, W
    BTFSS STATUS, 2 ; Z
    GOTO EPIC_IRQ_ClearFlag_L9
    ; phi copies for pred EPIC_IRQ_ClearFlag
    MOVLW 0x0B
    MOVWF 0x30
    MOVLW 0x00
    MOVWF 0x31
    GOTO EPIC_IRQ_ClearFlag_L14
EPIC_IRQ_ClearFlag_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2C
    MOVF 0x2C, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2D
    MOVF 0x2D, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp2
    MOVLW 0x0C
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
    GOTO tmp3
tmp2:
    MOVLW 0x0D
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
tmp3:
    ; phi copies for pred EPIC_IRQ_ClearFlag_L9
    MOVF 0x2E, W
    MOVWF 0x30
    MOVF 0x2F, W
    MOVWF 0x31
    GOTO EPIC_IRQ_ClearFlag_L14
EPIC_IRQ_ClearFlag_L14:
    BTFSC 0x31, 0
    BSF STATUS, 7
    BTFSS 0x31, 0
    BCF STATUS, 7
    MOVF 0x30, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x32
    MOVF 0x2A, W
    XORLW 0xFF
    MOVWF 0x33
    MOVF 0x33, W
    ANDWF 0x32, W
    MOVWF 0x34
    BTFSC 0x31, 0
    BSF STATUS, 7
    BTFSS 0x31, 0
    BCF STATUS, 7
    MOVF 0x30, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x34, W
    MOVWF INDF
    RETURN

read_offset:
    MOVF 0x25, W
    ANDLW 0x01
    MOVWF 0x26
    MOVF 0x26, W
    IORLW 0x0C
    MOVWF 0x27
    MOVF 0x27, W
    MOVWF 0x28
    CLRF 0x29
    MOVF 0x28, W
    MOVWF 0x2A
    MOVF 0x29, W
    MOVWF 0x2B
    BTFSC 0x2B, 0
    BSF STATUS, 7
    BTFSS 0x2B, 0
    BCF STATUS, 7
    MOVF 0x2A, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x2C
    MOVF 0x2C, W
    MOVWF 0x71
    RETURN

write_offset:
    MOVF 0x25, W
    ANDLW 0x01
    MOVWF 0x27
    MOVF 0x27, W
    IORLW 0x0C
    MOVWF 0x28
    MOVF 0x28, W
    MOVWF 0x29
    CLRF 0x2A
    MOVF 0x29, W
    MOVWF 0x2B
    MOVF 0x2A, W
    MOVWF 0x2C
    BTFSC 0x2C, 0
    BSF STATUS, 7
    BTFSS 0x2C, 0
    BCF STATUS, 7
    MOVF 0x2B, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x26, W
    MOVWF INDF
    RETURN

main:
    MOVF 0x20, W
    MOVWF 0x25
    MOVF 0x25, W
    ANDLW 0x07
    MOVWF 0x26
    MOVF 0x26, W
    MOVWF 0x27
    CLRF 0x28
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2A
    MOVF 0x29, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2B
    MOVF 0x2B, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L9
    ; phi copies for pred main
    MOVLW 0x0B
    MOVWF 0x30
    MOVLW 0x00
    MOVWF 0x31
    GOTO main_L14
main_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x27, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2C
    MOVF 0x2C, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2D
    MOVF 0x2D, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp4
    MOVLW 0x0C
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
    GOTO tmp5
tmp4:
    MOVLW 0x0D
    MOVWF 0x2E
    MOVLW 0x00
    MOVWF 0x2F
tmp5:
    ; phi copies for pred main_L9
    MOVF 0x2E, W
    MOVWF 0x30
    MOVF 0x2F, W
    MOVWF 0x31
    GOTO main_L14
main_L14:
    BTFSC 0x31, 0
    BSF STATUS, 7
    BTFSS 0x31, 0
    BCF STATUS, 7
    MOVF 0x30, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x32
    MOVF 0x2A, W
    ANDWF 0x32, W
    MOVWF 0x33
    MOVF 0x33, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x34
    MOVF 0x34, W
    MOVWF 0x35
    MOVF 0x35, W
    MOVWF 0x21
    MOVF 0x20, W
    MOVWF 0x36
    MOVF 0x36, W
    ANDLW 0x07
    MOVWF 0x37
    MOVF 0x37, W
    MOVWF 0x38
    CLRF 0x39
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x3A
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x3B
    MOVF 0x3A, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x3C
    MOVF 0x3C, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L28
    ; phi copies for pred main_L14
    MOVLW 0x0B
    MOVWF 0x41
    MOVLW 0x00
    MOVWF 0x42
    GOTO main_L33
main_L28:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x38, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x3D
    MOVF 0x3D, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x3E
    MOVF 0x3E, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp6
    MOVLW 0x0C
    MOVWF 0x3F
    MOVLW 0x00
    MOVWF 0x40
    GOTO tmp7
tmp6:
    MOVLW 0x0D
    MOVWF 0x3F
    MOVLW 0x00
    MOVWF 0x40
tmp7:
    ; phi copies for pred main_L28
    MOVF 0x3F, W
    MOVWF 0x41
    MOVF 0x40, W
    MOVWF 0x42
    GOTO main_L33
main_L33:
    BTFSC 0x42, 0
    BSF STATUS, 7
    BTFSS 0x42, 0
    BCF STATUS, 7
    MOVF 0x41, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x43
    MOVF 0x3B, W
    XORLW 0xFF
    MOVWF 0x44
    MOVF 0x44, W
    ANDWF 0x43, W
    MOVWF 0x45
    BTFSC 0x42, 0
    BSF STATUS, 7
    BTFSS 0x42, 0
    BCF STATUS, 7
    MOVF 0x41, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x45, W
    MOVWF INDF
    MOVF 0x0C, W
    MOVWF 0x46
    MOVF 0x0D, W
    MOVWF 0x47
    MOVF 0x46, W
    IORWF 0x47, W
    MOVWF 0x48
    MOVF 0x48, W
    MOVWF 0x22
    MOVF 0x21, W
    MOVWF 0x49
    MOVF 0x20, W
    MOVWF 0x4A
    MOVF 0x4A, W
    ANDLW 0x07
    MOVWF 0x4B
    MOVF 0x4B, W
    MOVWF 0x4C
    CLRF 0x4D
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x4E
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x4F
    MOVF 0x4E, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x50
    MOVF 0x50, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L50
    ; phi copies for pred main_L33
    MOVLW 0x0B
    MOVWF 0x55
    MOVLW 0x00
    MOVWF 0x56
    GOTO main_L55
main_L50:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4C, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x51
    MOVF 0x51, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x52
    MOVF 0x52, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp8
    MOVLW 0x0C
    MOVWF 0x53
    MOVLW 0x00
    MOVWF 0x54
    GOTO tmp9
tmp8:
    MOVLW 0x0D
    MOVWF 0x53
    MOVLW 0x00
    MOVWF 0x54
tmp9:
    ; phi copies for pred main_L50
    MOVF 0x53, W
    MOVWF 0x55
    MOVF 0x54, W
    MOVWF 0x56
    GOTO main_L55
main_L55:
    BTFSC 0x56, 0
    BSF STATUS, 7
    BTFSS 0x56, 0
    BCF STATUS, 7
    MOVF 0x55, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x57
    MOVF 0x4F, W
    ANDWF 0x57, W
    MOVWF 0x58
    MOVF 0x58, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x59
    MOVF 0x59, W
    MOVWF 0x5A
    MOVF 0x5A, W
    ADDWF 0x49, W
    MOVWF 0x5B
    MOVF 0x5B, W
    MOVWF 0x21
    MOVF 0x21, W
    MOVWF 0x5C
    MOVF 0x0C, W
    MOVWF 0x5D
    MOVF 0x5C, W
    ADDWF 0x5D, W
    MOVWF 0x5E
    MOVF 0x5E, W
    MOVWF 0x21
    MOVF 0x21, W
    MOVWF 0x5F
    MOVF 0x0D, W
    MOVWF 0x60
    MOVF 0x5F, W
    ADDWF 0x60, W
    MOVWF 0x61
    MOVF 0x61, W
    MOVWF 0x21
    MOVLW 0xAA
    MOVWF 0x0C
    MOVF 0x0C, W
    MOVWF 0x62
    MOVF 0x62, W
    MOVWF 0x23
    RETURN

__read_irq_table:
    MOVWF 0x70
    MOVLW HIGH(irq_table)
    MOVWF PCLATH
    MOVF 0x70, W
    ADDLW LOW(irq_table)
    MOVWF PCL
    .table irq_table 18
irq_table:
    RETLW 0x08
    RETLW 0x01
    RETLW 0x00
    RETLW 0x01
    RETLW 0x01
    RETLW 0x00
    RETLW 0x01
    RETLW 0x00
    RETLW 0x00
    RETLW 0x02
    RETLW 0x00
    RETLW 0x00
    RETLW 0x01
    RETLW 0x00
    RETLW 0x01
    RETLW 0x02
    RETLW 0x00
    RETLW 0x01

    end
