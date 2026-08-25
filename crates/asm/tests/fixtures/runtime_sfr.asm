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
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x24
    MOVF 0x24, W
    ANDLW 0x07
    MOVWF 0x25
    MOVF 0x25, W
    MOVWF 0x26
    CLRF 0x27
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x28
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVF 0x28, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2A
    MOVF 0x2A, W
    BTFSS STATUS, 2 ; Z
    GOTO EPIC_IRQ_GetFlag_L9
    ; phi copies for pred EPIC_IRQ_GetFlag
    MOVLW 0x0B
    MOVWF 0x2F
    MOVLW 0x00
    MOVWF 0x30
    GOTO EPIC_IRQ_GetFlag_L14
EPIC_IRQ_GetFlag_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2B
    MOVF 0x2B, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2C
    MOVF 0x2C, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVLW 0x0C
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
    GOTO tmp1
tmp0:
    MOVLW 0x0D
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
tmp1:
    ; phi copies for pred EPIC_IRQ_GetFlag_L9
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2D, W
    MOVWF 0x2F
    MOVF 0x2E, W
    MOVWF 0x30
    GOTO EPIC_IRQ_GetFlag_L14
EPIC_IRQ_GetFlag_L14:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x30, 0
    BSF STATUS, 7
    BTFSS 0x30, 0
    BCF STATUS, 7
    MOVF 0x2F, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x31
    MOVF 0x29, W
    ANDWF 0x31, W
    MOVWF 0x32
    MOVF 0x32, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x33
    MOVF 0x33, W
    MOVWF 0x34
    MOVF 0x34, W
    MOVWF 0x71
    RETURN

EPIC_IRQ_ClearFlag:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x24
    MOVF 0x24, W
    ANDLW 0x07
    MOVWF 0x25
    MOVF 0x25, W
    MOVWF 0x26
    CLRF 0x27
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x28
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVF 0x28, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2A
    MOVF 0x2A, W
    BTFSS STATUS, 2 ; Z
    GOTO EPIC_IRQ_ClearFlag_L9
    ; phi copies for pred EPIC_IRQ_ClearFlag
    MOVLW 0x0B
    MOVWF 0x2F
    MOVLW 0x00
    MOVWF 0x30
    GOTO EPIC_IRQ_ClearFlag_L14
EPIC_IRQ_ClearFlag_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2B
    MOVF 0x2B, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2C
    MOVF 0x2C, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp2
    MOVLW 0x0C
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
    GOTO tmp3
tmp2:
    MOVLW 0x0D
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
tmp3:
    ; phi copies for pred EPIC_IRQ_ClearFlag_L9
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2D, W
    MOVWF 0x2F
    MOVF 0x2E, W
    MOVWF 0x30
    GOTO EPIC_IRQ_ClearFlag_L14
EPIC_IRQ_ClearFlag_L14:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x30, 0
    BSF STATUS, 7
    BTFSS 0x30, 0
    BCF STATUS, 7
    MOVF 0x2F, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x31
    MOVF 0x29, W
    XORLW 0xFF
    MOVWF 0x32
    MOVF 0x32, W
    ANDWF 0x31, W
    MOVWF 0x33
    BTFSC 0x30, 0
    BSF STATUS, 7
    BTFSS 0x30, 0
    BCF STATUS, 7
    MOVF 0x2F, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x33, W
    MOVWF INDF
    RETURN

read_offset:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x24, W
    ANDLW 0x01
    MOVWF 0x25
    MOVF 0x25, W
    IORLW 0x0C
    MOVWF 0x26
    MOVF 0x26, W
    MOVWF 0x27
    CLRF 0x28
    MOVF 0x27, W
    MOVWF 0x29
    MOVF 0x28, W
    MOVWF 0x2A
    BTFSC 0x2A, 0
    BSF STATUS, 7
    BTFSS 0x2A, 0
    BCF STATUS, 7
    MOVF 0x29, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x2B
    MOVF 0x2B, W
    MOVWF 0x71
    RETURN

write_offset:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x24, W
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
    MOVF 0x25, W
    MOVWF INDF
    RETURN

main:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x24
    MOVF 0x24, W
    ANDLW 0x07
    MOVWF 0x25
    MOVF 0x25, W
    MOVWF 0x26
    CLRF 0x27
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x28
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x29
    MOVF 0x28, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2A
    MOVF 0x2A, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L9
    ; phi copies for pred main
    MOVLW 0x0B
    MOVWF 0x2F
    MOVLW 0x00
    MOVWF 0x30
    GOTO main_L14
main_L9:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x26, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x2B
    MOVF 0x2B, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x2C
    MOVF 0x2C, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp4
    MOVLW 0x0C
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
    GOTO tmp5
tmp4:
    MOVLW 0x0D
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x2D
    MOVLW 0x00
    MOVWF 0x2E
tmp5:
    ; phi copies for pred main_L9
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x2D, W
    MOVWF 0x2F
    MOVF 0x2E, W
    MOVWF 0x30
    GOTO main_L14
main_L14:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x30, 0
    BSF STATUS, 7
    BTFSS 0x30, 0
    BCF STATUS, 7
    MOVF 0x2F, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x31
    MOVF 0x29, W
    ANDWF 0x31, W
    MOVWF 0x32
    MOVF 0x32, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x33
    MOVF 0x33, W
    MOVWF 0x34
    MOVF 0x34, W
    MOVWF 0x21
    MOVF 0x20, W
    MOVWF 0x35
    MOVF 0x35, W
    ANDLW 0x07
    MOVWF 0x36
    MOVF 0x36, W
    MOVWF 0x37
    CLRF 0x38
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x39
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x3A
    MOVF 0x39, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x3B
    MOVF 0x3B, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L28
    ; phi copies for pred main_L14
    MOVLW 0x0B
    MOVWF 0x40
    MOVLW 0x00
    MOVWF 0x41
    GOTO main_L33
main_L28:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x37, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x3C
    MOVF 0x3C, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x3D
    MOVF 0x3D, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp6
    MOVLW 0x0C
    MOVWF 0x3E
    MOVLW 0x00
    MOVWF 0x3F
    GOTO tmp7
tmp6:
    MOVLW 0x0D
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x3E
    MOVLW 0x00
    MOVWF 0x3F
tmp7:
    ; phi copies for pred main_L28
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x3E, W
    MOVWF 0x40
    MOVF 0x3F, W
    MOVWF 0x41
    GOTO main_L33
main_L33:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x41, 0
    BSF STATUS, 7
    BTFSS 0x41, 0
    BCF STATUS, 7
    MOVF 0x40, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x42
    MOVF 0x3A, W
    XORLW 0xFF
    MOVWF 0x43
    MOVF 0x43, W
    ANDWF 0x42, W
    MOVWF 0x44
    BTFSC 0x41, 0
    BSF STATUS, 7
    BTFSS 0x41, 0
    BCF STATUS, 7
    MOVF 0x40, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x44, W
    MOVWF INDF
    MOVF 0x0C, W
    MOVWF 0x45
    MOVF 0x0D, W
    MOVWF 0x46
    MOVF 0x45, W
    IORWF 0x46, W
    MOVWF 0x47
    MOVF 0x47, W
    MOVWF 0x22
    MOVF 0x21, W
    MOVWF 0x48
    MOVF 0x20, W
    MOVWF 0x49
    MOVF 0x49, W
    ANDLW 0x07
    MOVWF 0x4A
    MOVF 0x4A, W
    MOVWF 0x4B
    CLRF 0x4C
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x01
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x4D
    MOVLW 0x00
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x00
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x4E
    MOVF 0x4D, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x4F
    MOVF 0x4F, W
    BTFSS STATUS, 2 ; Z
    GOTO main_L50
    ; phi copies for pred main_L33
    MOVLW 0x0B
    MOVWF 0x54
    MOVLW 0x00
    MOVWF 0x55
    GOTO main_L55
main_L50:
    MOVLW PAGE(__read_irq_table)
    MOVWF PCLATH
    MOVLW 0x00
    MOVWF 0x70
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x4B, W
    ADDWF 0x70, W
    MOVWF 0x70
    MOVF 0x70, W
    ADDLW 0x02
    CALL __read_irq_table
    MOVWF 0x70
    MOVF 0x70, W
    MOVWF 0x50
    MOVF 0x50, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x51
    MOVF 0x51, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp8
    MOVLW 0x0C
    MOVWF 0x52
    MOVLW 0x00
    MOVWF 0x53
    GOTO tmp9
tmp8:
    MOVLW 0x0D
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x52
    MOVLW 0x00
    MOVWF 0x53
tmp9:
    ; phi copies for pred main_L50
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x52, W
    MOVWF 0x54
    MOVF 0x53, W
    MOVWF 0x55
    GOTO main_L55
main_L55:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x55, 0
    BSF STATUS, 7
    BTFSS 0x55, 0
    BCF STATUS, 7
    MOVF 0x54, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x56
    MOVF 0x4E, W
    ANDWF 0x56, W
    MOVWF 0x57
    MOVF 0x57, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSS STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x58
    MOVF 0x58, W
    MOVWF 0x59
    MOVF 0x59, W
    ADDWF 0x48, W
    MOVWF 0x5A
    MOVF 0x5A, W
    MOVWF 0x21
    MOVF 0x21, W
    MOVWF 0x5B
    MOVF 0x20, W
    MOVWF 0x5C
    MOVF 0x5C, W
    ANDLW 0x01
    MOVWF 0x5D
    MOVF 0x5D, W
    IORLW 0x0C
    MOVWF 0x5E
    MOVF 0x5E, W
    MOVWF 0x5F
    CLRF 0x60
    MOVF 0x5F, W
    MOVWF 0x61
    MOVF 0x60, W
    MOVWF 0x62
    BTFSC 0x62, 0
    BSF STATUS, 7
    BTFSS 0x62, 0
    BCF STATUS, 7
    MOVF 0x61, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x63
    MOVF 0x5B, W
    ADDWF 0x63, W
    MOVWF 0x64
    MOVF 0x64, W
    MOVWF 0x21
    MOVF 0x21, W
    MOVWF 0x65
    MOVF 0x20, W
    MOVWF 0x66
    MOVF 0x66, W
    ANDLW 0x01
    MOVWF 0x67
    MOVF 0x67, W
    IORLW 0x0C
    MOVWF 0x68
    MOVF 0x68, W
    MOVWF 0x69
    CLRF 0x6A
    MOVF 0x69, W
    MOVWF 0x6B
    MOVF 0x6A, W
    MOVWF 0x6C
    BTFSC 0x6C, 0
    BSF STATUS, 7
    BTFSS 0x6C, 0
    BCF STATUS, 7
    MOVF 0x6B, W
    ADDLW 0x00
    MOVWF FSR
    MOVF INDF, W
    MOVWF 0x6D
    MOVF 0x65, W
    ADDWF 0x6D, W
    MOVWF 0x6E
    MOVF 0x6E, W
    MOVWF 0x21
    MOVF 0x20, W
    MOVWF 0x6F
    MOVF 0x6F, W
    ANDLW 0x01
    BSF STATUS, 5
    MOVWF 0x20
    MOVF 0x20, W
    IORLW 0x0C
    MOVWF 0x21
    MOVF 0x21, W
    MOVWF 0x22
    CLRF 0x23
    MOVF 0x22, W
    MOVWF 0x24
    MOVF 0x23, W
    MOVWF 0x25
    BTFSC 0x25, 0
    BSF STATUS, 7
    BTFSS 0x25, 0
    BCF STATUS, 7
    MOVF 0x24, W
    ADDLW 0x00
    MOVWF FSR
    MOVLW 0xAA
    MOVWF INDF
    BCF STATUS, 5
    MOVF 0x0C, W
    BSF STATUS, 5
    MOVWF 0x26
    MOVF 0x26, W
    BCF STATUS, 5
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
