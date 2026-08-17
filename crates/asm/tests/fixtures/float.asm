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
    MOVLW PAGE(main)
    MOVWF PCLATH
    CALL main
    SLEEP

half:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x51, W
    MOVWF 0x59
    MOVF 0x52, W
    MOVWF 0x5A
    MOVF 0x53, W
    MOVWF 0x5B
    MOVF 0x54, W
    MOVWF 0x5C
    MOVLW 0x00
    MOVWF 0x5D
    MOVLW 0x00
    MOVWF 0x5E
    MOVLW 0x20
    MOVWF 0x5F
    MOVLW 0x40
    MOVWF 0x60
    MOVLW PAGE(__div_f32)
    MOVWF PCLATH
    CALL __div_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x55
    MOVF 0x72, W
    MOVWF 0x56
    MOVF 0x73, W
    MOVWF 0x57
    MOVF 0x74, W
    MOVWF 0x58
    MOVF 0x55, W
    MOVWF 0x71
    MOVF 0x56, W
    MOVWF 0x72
    MOVF 0x57, W
    MOVWF 0x73
    MOVF 0x58, W
    MOVWF 0x74
    RETURN

pick:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    MOVWF 0x66
    MOVF 0x66, W
    XORLW 0x00
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x67
    MOVF 0x62, W
    MOVWF 0x68
    MOVF 0x63, W
    MOVWF 0x69
    MOVF 0x64, W
    MOVWF 0x6A
    MOVF 0x65, W
    MOVWF 0x6B
    MOVF 0x67, W
    BTFSC STATUS, 2 ; Z
    GOTO tmp0
    MOVF 0x68, W
    MOVWF 0x6C
    MOVF 0x69, W
    MOVWF 0x6D
    MOVF 0x6A, W
    MOVWF 0x6E
    MOVF 0x6B, W
    MOVWF 0x6F
    GOTO tmp1
tmp0:
    MOVLW 0x00
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x6C
    MOVLW 0x00
    MOVWF 0x6D
    MOVLW 0x00
    MOVWF 0x6E
    MOVLW 0x00
    MOVWF 0x6F
tmp1:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x6C, W
    MOVWF 0x71
    MOVF 0x6D, W
    MOVWF 0x72
    MOVF 0x6E, W
    MOVWF 0x73
    MOVF 0x6F, W
    MOVWF 0x74
    RETURN

mk:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x61, 0
    BSF STATUS, 7
    BTFSS 0x61, 0
    BCF STATUS, 7
    MOVF 0x60, W
    ADDLW 0x00
    MOVWF FSR
    MOVF 0x62, W
    MOVWF INDF
    BTFSC 0x61, 0
    BSF STATUS, 7
    BTFSS 0x61, 0
    BCF STATUS, 7
    MOVF 0x60, W
    ADDLW 0x02
    MOVWF FSR
    MOVF 0x63, W
    MOVWF INDF
    BTFSC 0x61, 0
    BSF STATUS, 7
    BTFSS 0x61, 0
    BCF STATUS, 7
    MOVF 0x60, W
    ADDLW 0x03
    MOVWF FSR
    MOVF 0x64, W
    MOVWF INDF
    BTFSC 0x61, 0
    BSF STATUS, 7
    BTFSS 0x61, 0
    BCF STATUS, 7
    MOVF 0x60, W
    ADDLW 0x04
    MOVWF FSR
    MOVF 0x65, W
    MOVWF INDF
    BTFSC 0x61, 0
    BSF STATUS, 7
    BTFSS 0x61, 0
    BCF STATUS, 7
    MOVF 0x60, W
    ADDLW 0x05
    MOVWF FSR
    MOVF 0x66, W
    MOVWF INDF
    RETURN

struct_step:
    MOVLW 0x56
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x60
    MOVLW 0x00
    MOVWF 0x61
    MOVF 0x51, W
    MOVWF 0x62
    MOVF 0x52, W
    MOVWF 0x63
    MOVF 0x53, W
    MOVWF 0x64
    MOVF 0x54, W
    MOVWF 0x65
    MOVF 0x55, W
    MOVWF 0x66
    MOVLW PAGE(mk)
    MOVWF PCLATH
    CALL mk
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x56, W
    MOVWF 0x60
    MOVF 0x57, W
    MOVWF 0x61
    MOVF 0x58, W
    MOVWF 0x62
    MOVF 0x59, W
    MOVWF 0x63
    MOVF 0x5A, W
    MOVWF 0x64
    MOVF 0x5B, W
    MOVWF 0x65
    MOVLW PAGE(pick)
    MOVWF PCLATH
    CALL pick
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x5C
    MOVF 0x72, W
    MOVWF 0x5D
    MOVF 0x73, W
    MOVWF 0x5E
    MOVF 0x74, W
    MOVWF 0x5F
    MOVF 0x5C, W
    MOVWF 0x71
    MOVF 0x5D, W
    MOVWF 0x72
    MOVF 0x5E, W
    MOVWF 0x73
    MOVF 0x5F, W
    MOVWF 0x74
    RETURN

main:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x20, W
    MOVWF 0x30
    MOVF 0x21, W
    MOVWF 0x31
    MOVF 0x22, W
    MOVWF 0x32
    MOVF 0x23, W
    MOVWF 0x33
    MOVF 0x30, W
    MOVWF 0x51
    MOVF 0x31, W
    MOVWF 0x52
    MOVF 0x32, W
    MOVWF 0x53
    MOVF 0x33, W
    MOVWF 0x54
    MOVLW PAGE(half)
    MOVWF PCLATH
    CALL half
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x34
    MOVF 0x72, W
    MOVWF 0x35
    MOVF 0x73, W
    MOVWF 0x36
    MOVF 0x74, W
    MOVWF 0x37
    MOVF 0x34, W
    MOVWF 0x24
    MOVF 0x35, W
    MOVWF 0x25
    MOVF 0x36, W
    MOVWF 0x26
    MOVF 0x37, W
    MOVWF 0x27
    MOVF 0x30, W
    MOVWF 0x51
    MOVF 0x31, W
    MOVWF 0x52
    MOVF 0x32, W
    MOVWF 0x53
    MOVF 0x33, W
    MOVWF 0x54
    MOVLW 0x00
    MOVWF 0x55
    MOVLW 0x00
    MOVWF 0x56
    MOVLW 0x80
    MOVWF 0x57
    MOVLW 0x3E
    MOVWF 0x58
    MOVLW PAGE(__add_f32)
    MOVWF PCLATH
    CALL __add_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x38
    MOVF 0x72, W
    MOVWF 0x39
    MOVF 0x73, W
    MOVWF 0x3A
    MOVF 0x74, W
    MOVWF 0x3B
    MOVF 0x38, W
    MOVWF 0x51
    MOVF 0x39, W
    MOVWF 0x52
    MOVF 0x3A, W
    MOVWF 0x53
    MOVF 0x3B, W
    MOVWF 0x54
    MOVLW 0x00
    MOVWF 0x55
    MOVLW 0x00
    MOVWF 0x56
    MOVLW 0x40
    MOVWF 0x57
    MOVLW 0x40
    MOVWF 0x58
    MOVLW PAGE(__mul_f32)
    MOVWF PCLATH
    CALL __mul_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x3C
    MOVF 0x72, W
    MOVWF 0x3D
    MOVF 0x73, W
    MOVWF 0x3E
    MOVF 0x74, W
    MOVWF 0x3F
    MOVF 0x3C, W
    MOVWF 0x51
    MOVF 0x3D, W
    MOVWF 0x52
    MOVF 0x3E, W
    MOVWF 0x53
    MOVF 0x3F, W
    MOVWF 0x54
    MOVLW PAGE(__fptosi_f32)
    MOVWF PCLATH
    CALL __fptosi_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x40
    MOVF 0x72, W
    MOVWF 0x41
    MOVF 0x40, W
    MOVWF 0x51
    MOVF 0x41, W
    MOVWF 0x52
    MOVF 0x52, W
    MOVWF 0x53
    MOVWF 0x54
    MOVLW PAGE(__sitofp_f32)
    MOVWF PCLATH
    CALL __sitofp_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x42
    MOVF 0x72, W
    MOVWF 0x43
    MOVF 0x73, W
    MOVWF 0x44
    MOVF 0x74, W
    MOVWF 0x45
    MOVF 0x42, W
    MOVWF 0x28
    MOVF 0x43, W
    MOVWF 0x29
    MOVF 0x44, W
    MOVWF 0x2A
    MOVF 0x45, W
    MOVWF 0x2B
    MOVF 0x30, W
    MOVWF 0x51
    MOVF 0x31, W
    MOVWF 0x52
    MOVF 0x32, W
    MOVWF 0x53
    MOVF 0x33, W
    MOVWF 0x54
    MOVLW 0x00
    MOVWF 0x55
    MOVLW 0x00
    MOVWF 0x56
    MOVLW 0x40
    MOVWF 0x57
    MOVLW 0x3F
    MOVWF 0x58
    MOVLW PAGE(__cmp_f32)
    MOVWF PCLATH
    CALL __cmp_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x46
    MOVF 0x46, W
    XORLW 0x01
    MOVWF 0x70
    MOVLW 0x00
    BTFSC STATUS, 2 ; Z
    MOVLW 0x01
    MOVWF 0x47
    MOVF 0x47, W
    MOVWF 0x48
    MOVLW 0x00
    MOVWF 0x59
    MOVLW 0x00
    MOVWF 0x5A
    MOVLW 0x80
    MOVWF 0x5B
    MOVLW 0x3F
    MOVWF 0x5C
    MOVF 0x30, W
    MOVWF 0x5D
    MOVF 0x31, W
    MOVWF 0x5E
    MOVF 0x32, W
    MOVWF 0x5F
    MOVF 0x33, W
    MOVWF 0x60
    MOVLW PAGE(__div_f32)
    MOVWF PCLATH
    CALL __div_f32
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x49
    MOVF 0x72, W
    MOVWF 0x4A
    MOVF 0x73, W
    MOVWF 0x4B
    MOVF 0x74, W
    MOVWF 0x4C
    MOVF 0x48, W
    MOVWF 0x51
    MOVF 0x49, W
    MOVWF 0x52
    MOVF 0x4A, W
    MOVWF 0x53
    MOVF 0x4B, W
    MOVWF 0x54
    MOVF 0x4C, W
    MOVWF 0x55
    MOVLW PAGE(struct_step)
    MOVWF PCLATH
    CALL struct_step
    MOVF 0x71, W
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x4D
    MOVF 0x72, W
    MOVWF 0x4E
    MOVF 0x73, W
    MOVWF 0x4F
    MOVF 0x74, W
    MOVWF 0x50
    MOVF 0x4D, W
    MOVWF 0x2C
    MOVF 0x4E, W
    MOVWF 0x2D
    MOVF 0x4F, W
    MOVWF 0x2E
    MOVF 0x50, W
    MOVWF 0x2F
    RETURN

__div_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5C, W
    XORWF 0x60, W
    ANDLW 0x80
    MOVWF 0x61
    MOVF 0x5C, W
    ANDLW 0x7F
    MOVWF 0x6C
    BCF STATUS, 0
    RLF 0x6C, F
    BTFSC 0x5B, 7
    BSF 0x6C, 0
    MOVF 0x60, W
    ANDLW 0x7F
    MOVWF 0x62
    BCF STATUS, 0
    RLF 0x62, F
    BTFSC 0x5F, 7
    BSF 0x62, 0
    MOVF 0x62, W
    SUBWF 0x6C, W
    MOVWF 0x6C
    CLRF 0x67
    BTFSS STATUS, 0
    BSF 0x67, 0
    ADDLW 0x7F
    MOVWF 0x62
    MOVLW 0x00
    BTFSS STATUS, 0
    GOTO tmp15
    BTFSC 0x67, 0
    GOTO tmp16
    MOVLW 0x01
    GOTO tmp16
tmp15:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x67, 0
    GOTO tmp16
    MOVLW 0xFF
tmp16:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x63
    MOVF 0x5C, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp17
    BTFSC 0x5B, 7
    GOTO tmp17
tmp2:
    CLRF 0x71
    CLRF 0x72
    CLRF 0x73
    CLRF 0x74
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x61, 7
    BSF 0x74, 7
    RETURN
tmp17:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp18
    BTFSC 0x5F, 7
    GOTO tmp18
tmp3:
    CLRF 0x71
    CLRF 0x72
    MOVLW 0x80
    MOVWF 0x73
    MOVLW 0x7F
    MOVWF 0x74
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x61, 7
    BSF 0x74, 7
    RETURN
tmp18:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5B, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x5B
    MOVF 0x5D, W
    MOVWF 0x68
    MOVF 0x5E, W
    MOVWF 0x69
    MOVF 0x5F, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x6A
    CLRF 0x64
    CLRF 0x65
    CLRF 0x66
    CLRF 0x67
    MOVLW 0x18
    MOVWF 0x6B
tmp4:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x59, F
    RLF 0x5A, F
    RLF 0x5B, F
    RLF 0x64, F
    RLF 0x65, F
    RLF 0x66, F
    RLF 0x67, F
    MOVF 0x68, W
    SUBWF 0x64, F
    MOVF 0x69, W
    BTFSS STATUS, 0
    INCFSZ 0x69, W
    SUBWF 0x65, F
    MOVF 0x6A, W
    BTFSS STATUS, 0
    INCFSZ 0x6A, W
    SUBWF 0x66, F
    MOVLW 0x00
    BTFSS STATUS, 0
    ADDLW 0x01
    SUBWF 0x67, F
    BTFSS STATUS, 0
    GOTO tmp5
    BSF 0x59, 0
    GOTO tmp6
tmp5:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x68, W
    ADDWF 0x64, F
    MOVF 0x69, W
    BTFSC STATUS, 0
    INCFSZ 0x69, W
    ADDWF 0x65, F
    MOVF 0x6A, W
    BTFSC STATUS, 0
    INCFSZ 0x6A, W
    ADDWF 0x66, F
    MOVLW 0x00
    BTFSC STATUS, 0
    ADDLW 0x01
    ADDWF 0x67, F
tmp6:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x6B, F
    GOTO tmp4
    CLRF 0x6C
    MOVF 0x59, W
    ANDLW 0x01
    BTFSC STATUS, 2
    GOTO tmp8
    BSF 0x6C, 0
tmp8:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x59
    CLRF 0x5A
    CLRF 0x5B
    CLRF 0x5C
    MOVLW 0x19
    MOVWF 0x6B
tmp9:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x59, F
    RLF 0x5A, F
    RLF 0x5B, F
    RLF 0x5C, F
    BCF STATUS, 0
    RLF 0x64, F
    RLF 0x65, F
    RLF 0x66, F
    RLF 0x67, F
    MOVF 0x68, W
    SUBWF 0x64, F
    MOVF 0x69, W
    BTFSS STATUS, 0
    INCFSZ 0x69, W
    SUBWF 0x65, F
    MOVF 0x6A, W
    BTFSS STATUS, 0
    INCFSZ 0x6A, W
    SUBWF 0x66, F
    MOVLW 0x00
    BTFSS STATUS, 0
    ADDLW 0x01
    SUBWF 0x67, F
    BTFSS STATUS, 0
    GOTO tmp10
    BSF 0x59, 0
    GOTO tmp11
tmp10:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x68, W
    ADDWF 0x64, F
    MOVF 0x69, W
    BTFSC STATUS, 0
    INCFSZ 0x69, W
    ADDWF 0x65, F
    MOVF 0x6A, W
    BTFSC STATUS, 0
    INCFSZ 0x6A, W
    ADDWF 0x66, F
    MOVLW 0x00
    BTFSC STATUS, 0
    ADDLW 0x01
    ADDWF 0x67, F
tmp11:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x6B, F
    GOTO tmp9
    BTFSC 0x6C, 0
    GOTO tmp7
    MOVLW 0x01
    SUBWF 0x62, F
    BCF STATUS, 0
    BTFSC 0x5C, 0
    BSF STATUS, 0
    RRF 0x5B, F
    RRF 0x5A, F
    RRF 0x59, F
    CLRF 0x6C
    BTFSC STATUS, 0
    BSF 0x6C, 0
    MOVF 0x64, W
    IORWF 0x65, W
    IORWF 0x66, W
    IORWF 0x67, W
    BTFSC STATUS, 2
    GOTO tmp12
    BSF 0x6C, 1
    GOTO tmp12
tmp7:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x5C, 0
    BSF STATUS, 0
    RRF 0x5B, F
    RRF 0x5A, F
    RRF 0x59, F
    CLRF 0x6C
    BTFSC STATUS, 0
    BSF 0x6C, 1
    BCF STATUS, 0
    RRF 0x5B, F
    RRF 0x5A, F
    RRF 0x59, F
    BTFSC STATUS, 0
    BSF 0x6C, 0
    BSF 0x5B, 7
    MOVF 0x64, W
    IORWF 0x65, W
    IORWF 0x66, W
    IORWF 0x67, W
    BTFSC STATUS, 2
    GOTO tmp12
    BSF 0x6C, 1
tmp12:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x6C, 0
    GOTO tmp14
    BTFSC 0x6C, 1
    GOTO tmp13
    BTFSC 0x59, 0
    GOTO tmp13
    GOTO tmp14
tmp13:
    BCF STATUS, 5
    BCF STATUS, 6
    INCF 0x59, F
    BTFSC STATUS, 2
    INCF 0x5A, F
    BTFSC STATUS, 2
    INCF 0x5B, F
    BTFSC STATUS, 2
    GOTO tmp19
    GOTO tmp20
tmp19:
    MOVLW 0x80
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x5B
    CLRF 0x5A
    CLRF 0x59
    MOVLW 0x01
    ADDWF 0x62, F
tmp20:
tmp14:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x59, W
    MOVWF 0x71
    MOVF 0x5A, W
    MOVWF 0x72
    MOVLW 0x7F
    ANDWF 0x5B, W
    MOVWF 0x73
    BTFSC 0x62, 0
    BSF 0x73, 7
    MOVF 0x62, W
    MOVWF 0x74
    BCF STATUS, 0
    RRF 0x74, F
    BTFSC 0x61, 7
    BSF 0x74, 7
    RETURN
__add_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    ANDLW 0x80
    MOVWF 0x59
    MOVF 0x54, W
    ANDLW 0x7F
    MOVWF 0x5A
    BCF STATUS, 0
    RLF 0x5A, F
    BTFSC 0x53, 7
    BSF 0x5A, 0
    MOVF 0x51, W
    MOVWF 0x5B
    MOVF 0x52, W
    MOVWF 0x5C
    MOVF 0x53, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x5D
    MOVF 0x5A, W
    BTFSS STATUS, 2
    GOTO tmp21
    CLRF 0x5B
    CLRF 0x5C
    CLRF 0x5D
tmp21:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x58, W
    ANDLW 0x80
    MOVWF 0x5E
    MOVF 0x58, W
    ANDLW 0x7F
    MOVWF 0x5F
    BCF STATUS, 0
    RLF 0x5F, F
    BTFSC 0x57, 7
    BSF 0x5F, 0
    MOVF 0x55, W
    MOVWF 0x60
    MOVF 0x56, W
    MOVWF 0x61
    MOVF 0x57, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x62
    MOVF 0x5F, W
    BTFSS STATUS, 2
    GOTO tmp22
    CLRF 0x60
    CLRF 0x61
    CLRF 0x62
tmp22:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5B, W
    IORWF 0x5C, W
    IORWF 0x5D, W
    BTFSS STATUS, 2
    GOTO tmp23
    MOVF 0x60, W
    IORWF 0x61, W
    IORWF 0x62, W
    BTFSS STATUS, 2
    GOTO tmp24
    GOTO tmp25
tmp24:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5E, W
    MOVWF 0x59
    MOVF 0x5F, W
    MOVWF 0x5A
    MOVF 0x60, W
    MOVWF 0x5B
    MOVF 0x61, W
    MOVWF 0x5C
    MOVF 0x62, W
    MOVWF 0x5D
    CLRF 0x63
    GOTO tmp40
tmp23:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    IORWF 0x61, W
    IORWF 0x62, W
    BTFSS STATUS, 2
    GOTO tmp26
    CLRF 0x63
    GOTO tmp40
tmp26:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x63
    MOVF 0x5F, W
    SUBWF 0x5A, W
    BTFSS STATUS, 0
    GOTO tmp27
    MOVF 0x5E, W
    XORWF 0x59, F
    MOVF 0x59, W
    XORWF 0x5E, F
    MOVF 0x5E, W
    XORWF 0x59, F
    MOVF 0x5F, W
    XORWF 0x5A, F
    MOVF 0x5A, W
    XORWF 0x5F, F
    MOVF 0x5F, W
    XORWF 0x5A, F
    MOVF 0x60, W
    XORWF 0x5B, F
    MOVF 0x5B, W
    XORWF 0x60, F
    MOVF 0x60, W
    XORWF 0x5B, F
    MOVF 0x61, W
    XORWF 0x5C, F
    MOVF 0x5C, W
    XORWF 0x61, F
    MOVF 0x61, W
    XORWF 0x5C, F
    MOVF 0x62, W
    XORWF 0x5D, F
    MOVF 0x5D, W
    XORWF 0x62, F
    MOVF 0x62, W
    XORWF 0x5D, F
tmp27:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5A, W
    SUBWF 0x5F, W
    MOVWF 0x64
    MOVF 0x5F, W
    MOVWF 0x5A
    MOVLW 0x1F
    SUBWF 0x64, W
    BTFSS STATUS, 0
    GOTO tmp28
    MOVLW 0x1F
    MOVWF 0x64
tmp28:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x64, W
    BTFSS STATUS, 2
    GOTO tmp29
    GOTO tmp30
tmp29:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x63, 0
    BSF 0x63, 1
    BCF STATUS, 0
    RRF 0x5D, F
    RRF 0x5C, F
    RRF 0x5B, F
    BCF 0x63, 0
    BTFSC STATUS, 0
    BSF 0x63, 0
    DECFSZ 0x64, F
    GOTO tmp29
tmp30:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x59, W
    XORWF 0x5E, W
    BTFSS STATUS, 2
    GOTO tmp31
    MOVF 0x60, W
    ADDWF 0x5B, F
    MOVF 0x61, W
    BTFSC STATUS, 0
    INCFSZ 0x61, W
    ADDWF 0x5C, F
    MOVF 0x62, W
    BTFSC STATUS, 0
    INCFSZ 0x62, W
    ADDWF 0x5D, F
    BTFSC STATUS, 0
    GOTO tmp32
    GOTO tmp38
tmp32:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x63, 0
    BSF 0x63, 1
    BCF STATUS, 0
    RRF 0x5D, F
    RRF 0x5C, F
    RRF 0x5B, F
    BCF 0x63, 0
    BTFSC STATUS, 0
    BSF 0x63, 0
    BSF 0x5D, 7
    MOVLW 0x01
    ADDWF 0x5A, F
    GOTO tmp38
tmp31:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x62, W
    SUBWF 0x5D, W
    BTFSS STATUS, 0
    GOTO tmp35
    BTFSC STATUS, 2
    GOTO tmp33
    GOTO tmp36
tmp33:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x61, W
    SUBWF 0x5C, W
    BTFSS STATUS, 0
    GOTO tmp35
    BTFSC STATUS, 2
    GOTO tmp34
    GOTO tmp36
tmp34:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    SUBWF 0x5B, W
    BTFSS STATUS, 0
    GOTO tmp35
    BTFSC STATUS, 2
    GOTO tmp25
    GOTO tmp36
tmp35:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    XORWF 0x5B, F
    MOVF 0x5B, W
    XORWF 0x60, F
    MOVF 0x60, W
    XORWF 0x5B, F
    MOVF 0x61, W
    XORWF 0x5C, F
    MOVF 0x5C, W
    XORWF 0x61, F
    MOVF 0x61, W
    XORWF 0x5C, F
    MOVF 0x62, W
    XORWF 0x5D, F
    MOVF 0x5D, W
    XORWF 0x62, F
    MOVF 0x62, W
    XORWF 0x5D, F
    MOVF 0x5E, W
    MOVWF 0x59
tmp36:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    SUBWF 0x5B, F
    MOVF 0x61, W
    BTFSS STATUS, 0
    INCFSZ 0x61, W
    SUBWF 0x5C, F
    MOVF 0x62, W
    BTFSS STATUS, 0
    INCFSZ 0x62, W
    SUBWF 0x5D, F
tmp37:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5D, W
    ANDLW 0x80
    BTFSS STATUS, 2
    GOTO tmp38
    MOVF 0x5A, W
    BTFSC STATUS, 2
    GOTO tmp38
    BCF STATUS, 0
    BTFSC 0x63, 0
    BSF STATUS, 0
    BTFSS 0x63, 0
    BCF STATUS, 0
    RLF 0x5B, F
    RLF 0x5C, F
    RLF 0x5D, F
    BTFSC 0x63, 1
    BSF 0x63, 0
    BTFSS 0x63, 1
    BCF 0x63, 0
    MOVLW 0x01
    SUBWF 0x5A, F
    GOTO tmp37
tmp38:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x63, 0
    GOTO tmp40
    BTFSC 0x63, 1
    GOTO tmp39
    BTFSC 0x5B, 0
    GOTO tmp39
    GOTO tmp40
tmp39:
    BCF STATUS, 5
    BCF STATUS, 6
    INCF 0x5B, F
    BTFSC STATUS, 2
    INCF 0x5C, F
    BTFSC STATUS, 2
    INCF 0x5D, F
    BTFSC STATUS, 2
    GOTO tmp43
    GOTO tmp44
tmp43:
    MOVLW 0x80
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x5D
    CLRF 0x5C
    CLRF 0x5B
    MOVLW 0x01
    ADDWF 0x5A, F
tmp44:
tmp40:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5B, W
    MOVWF 0x71
    MOVF 0x5C, W
    MOVWF 0x72
    MOVLW 0x7F
    ANDWF 0x5D, W
    MOVWF 0x73
    BTFSC 0x5A, 0
    BSF 0x73, 7
    MOVF 0x5A, W
    MOVWF 0x74
    BCF STATUS, 0
    RRF 0x74, F
    BTFSC 0x59, 7
    BSF 0x74, 7
    RETURN
tmp25:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x59, 7
    GOTO tmp42
    BTFSS 0x5E, 7
    GOTO tmp41
    GOTO tmp42
tmp41:
    BCF STATUS, 5
    BCF STATUS, 6
    BCF 0x59, 7
tmp42:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x5A
    CLRF 0x5B
    CLRF 0x5C
    CLRF 0x5D
    GOTO tmp40
__mul_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    XORWF 0x58, W
    ANDLW 0x80
    MOVWF 0x59
    MOVF 0x54, W
    ANDLW 0x7F
    MOVWF 0x64
    BCF STATUS, 0
    RLF 0x64, F
    BTFSC 0x53, 7
    BSF 0x64, 0
    MOVF 0x58, W
    ANDLW 0x7F
    MOVWF 0x65
    BCF STATUS, 0
    RLF 0x65, F
    BTFSC 0x57, 7
    BSF 0x65, 0
    MOVF 0x65, W
    ADDWF 0x64, W
    MOVWF 0x64
    CLRF 0x63
    BTFSC STATUS, 0
    BSF 0x63, 0
    MOVLW 0x81
    ADDWF 0x64, W
    MOVWF 0x5A
    MOVLW 0x00
    BTFSS STATUS, 0
    GOTO tmp50
    BTFSC 0x63, 0
    MOVLW 0x01
    GOTO tmp51
tmp50:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x63, 0
    GOTO tmp51
    MOVLW 0xFF
tmp51:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x5B
    MOVF 0x54, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp52
    BTFSC 0x53, 7
    GOTO tmp52
    GOTO tmp54
tmp52:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x58, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp53
    BTFSC 0x57, 7
    GOTO tmp53
    GOTO tmp54
tmp53:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x53, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x53
    MOVF 0x55, W
    MOVWF 0x5C
    MOVF 0x56, W
    MOVWF 0x5D
    MOVF 0x57, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x5E
    MOVF 0x57, W
    ANDLW 0x7F
    MOVWF 0x57
    CLRF 0x60
    CLRF 0x61
    CLRF 0x62
    CLRF 0x63
    CLRF 0x64
    CLRF 0x65
    CLRF 0x66
    MOVLW 0x18
    MOVWF 0x5F
tmp45:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5C, F
    RLF 0x5D, F
    RLF 0x5E, F
    BTFSS STATUS, 0
    GOTO tmp46
    MOVF 0x55, W
    ADDWF 0x64, F
    MOVF 0x56, W
    BTFSC STATUS, 0
    INCFSZ 0x56, W
    ADDWF 0x65, F
    MOVF 0x57, W
    BTFSC STATUS, 0
    INCFSZ 0x57, W
    ADDWF 0x66, F
    MOVF 0x51, W
    ADDWF 0x60, F
    MOVF 0x52, W
    BTFSC STATUS, 0
    INCFSZ 0x52, W
    ADDWF 0x61, F
    MOVF 0x53, W
    BTFSC STATUS, 0
    INCFSZ 0x53, W
    ADDWF 0x62, F
    MOVLW 0x00
    BTFSC STATUS, 0
    ADDLW 0x01
    ADDWF 0x63, F
tmp46:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RRF 0x53, F
    RRF 0x52, F
    RRF 0x51, F
    BCF STATUS, 0
    RLF 0x55, F
    RLF 0x56, F
    RLF 0x57, F
    DECFSZ 0x5F, F
    GOTO tmp45
    BTFSC 0x63, 0
    GOTO tmp47
    BTFSS 0x66, 6
    GOTO tmp49
    MOVF 0x64, W
    IORWF 0x65, W
    IORWF 0x66, W
    ANDLW 0x3F
    BTFSS STATUS, 2
    GOTO tmp48
    BTFSC 0x60, 0
    GOTO tmp48
    GOTO tmp49
tmp47:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x63, 0
    BSF STATUS, 0
    RRF 0x62, F
    RRF 0x61, F
    RRF 0x60, F
    BCF 0x66, 7
    BTFSC STATUS, 0
    BSF 0x66, 7
    MOVLW 0x01
    ADDWF 0x5A, F
    BTFSS 0x66, 7
    GOTO tmp49
    MOVLW 0xBF
    ANDWF 0x66, W
    IORWF 0x65, W
    IORWF 0x64, W
    BTFSS STATUS, 2
    GOTO tmp48
    BTFSC 0x60, 0
    GOTO tmp48
    GOTO tmp49
tmp48:
    BCF STATUS, 5
    BCF STATUS, 6
    INCF 0x60, F
    BTFSC STATUS, 2
    INCF 0x61, F
    BTFSC STATUS, 2
    INCF 0x62, F
    BTFSC STATUS, 2
    GOTO tmp55
    GOTO tmp56
tmp55:
    MOVLW 0x80
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x62
    CLRF 0x61
    CLRF 0x60
    MOVLW 0x01
    ADDWF 0x5A, F
tmp56:
tmp49:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x60, W
    MOVWF 0x71
    MOVF 0x61, W
    MOVWF 0x72
    MOVLW 0x7F
    ANDWF 0x62, W
    MOVWF 0x73
    BTFSC 0x5A, 0
    BSF 0x73, 7
    MOVF 0x5A, W
    MOVWF 0x74
    BCF STATUS, 0
    RRF 0x74, F
    BTFSC 0x59, 7
    BSF 0x74, 7
    RETURN
tmp54:
    CLRF 0x71
    CLRF 0x72
    CLRF 0x73
    CLRF 0x74
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x59, 7
    BSF 0x74, 7
    RETURN
__fptosi_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    ANDLW 0x7F
    MOVWF 0x55
    BCF STATUS, 0
    RLF 0x55, F
    BTFSC 0x53, 7
    BSF 0x55, 0
    MOVF 0x54, W
    ANDLW 0x80
    MOVWF 0x5A
    MOVF 0x55, W
    BTFSS STATUS, 2
    GOTO tmp57
    CLRF 0x57
    CLRF 0x58
    CLRF 0x59
    CLRF 0x5B
    GOTO tmp63
tmp57:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x51, W
    MOVWF 0x57
    MOVF 0x52, W
    MOVWF 0x58
    MOVF 0x53, W
    ANDLW 0x7F
    IORLW 0x80
    MOVWF 0x59
    CLRF 0x5B
    MOVF 0x55, W
    SUBLW 0x96
    BTFSS STATUS, 0
    GOTO tmp58
    MOVWF 0x56
    MOVLW 0x1F
    SUBWF 0x56, W
    BTFSS STATUS, 0
    GOTO tmp59
    MOVLW 0x1F
    MOVWF 0x56
tmp59:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x56, W
    BTFSS STATUS, 2
    GOTO tmp60
    GOTO tmp63
tmp60:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RRF 0x59, F
    RRF 0x58, F
    RRF 0x57, F
    DECFSZ 0x56, F
    GOTO tmp60
    GOTO tmp63
tmp58:
    SUBLW 0x00
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x56
    MOVLW 0x08
    SUBWF 0x56, W
    BTFSS STATUS, 0
    GOTO tmp61
    BTFSS 0x5A, 7
    GOTO tmp62
    CLRF 0x57
    CLRF 0x58
    CLRF 0x59
    MOVLW 0x80
    MOVWF 0x5B
    GOTO tmp63
tmp62:
    MOVLW 0xFF
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x57
    MOVWF 0x58
    MOVWF 0x59
    MOVLW 0x7F
    MOVWF 0x5B
    GOTO tmp63
tmp61:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x57, F
    RLF 0x58, F
    RLF 0x59, F
    RLF 0x5B, F
    DECFSZ 0x56, F
    GOTO tmp61
tmp63:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x5A, 7
    GOTO tmp64
    COMF 0x57, F
    COMF 0x58, F
    COMF 0x59, F
    COMF 0x5B, F
    INCF 0x57, F
    BTFSC STATUS, 2
    INCF 0x58, F
    BTFSC STATUS, 2
    INCF 0x59, F
    BTFSC STATUS, 2
    INCF 0x5B, F
tmp64:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x57, W
    MOVWF 0x71
    MOVF 0x58, W
    MOVWF 0x72
    MOVF 0x59, W
    MOVWF 0x73
    MOVF 0x5B, W
    MOVWF 0x74
    RETURN
__sitofp_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    ANDLW 0x80
    MOVWF 0x5A
    BTFSS 0x54, 7
    GOTO tmp65
    COMF 0x51, F
    COMF 0x52, F
    COMF 0x53, F
    COMF 0x54, F
    INCF 0x51, F
    BTFSC STATUS, 2
    INCF 0x52, F
    BTFSC STATUS, 2
    INCF 0x53, F
    BTFSC STATUS, 2
    INCF 0x54, F
tmp65:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x51, W
    IORWF 0x52, W
    IORWF 0x53, W
    IORWF 0x54, W
    BTFSS STATUS, 2
    GOTO tmp67
    CLRF 0x71
    CLRF 0x72
    CLRF 0x73
    CLRF 0x74
    BTFSC 0x5A, 7
    BSF 0x74, 7
    RETURN
tmp67:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x55
tmp68:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSC 0x54, 7
    GOTO tmp66
    BCF STATUS, 0
    RLF 0x51, F
    RLF 0x52, F
    RLF 0x53, F
    RLF 0x54, F
    INCF 0x55, F
    GOTO tmp68
tmp66:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x55, W
    SUBLW 0x9E
    MOVWF 0x56
    CLRF 0x57
    MOVF 0x51, W
    ANDLW 0x80
    MOVWF 0x58
    MOVF 0x51, W
    ANDLW 0x7F
    MOVWF 0x59
    BTFSS 0x58, 7
    GOTO tmp70
    MOVF 0x59, W
    BTFSS STATUS, 2
    GOTO tmp69
    BTFSC 0x52, 0
    GOTO tmp69
    GOTO tmp70
tmp69:
    BCF STATUS, 5
    BCF STATUS, 6
    INCF 0x52, F
    BTFSC STATUS, 2
    INCF 0x53, F
    BTFSC STATUS, 2
    INCF 0x54, F
    BTFSC STATUS, 2
    GOTO tmp71
    GOTO tmp72
tmp71:
    MOVLW 0x80
    BCF STATUS, 5
    BCF STATUS, 6
    MOVWF 0x54
    CLRF 0x53
    CLRF 0x52
    MOVLW 0x01
    ADDWF 0x56, F
tmp72:
tmp70:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x52, W
    MOVWF 0x71
    MOVF 0x53, W
    MOVWF 0x72
    MOVLW 0x7F
    ANDWF 0x54, W
    MOVWF 0x73
    BTFSC 0x56, 0
    BSF 0x73, 7
    MOVF 0x56, W
    MOVWF 0x74
    BCF STATUS, 0
    RRF 0x74, F
    BTFSC 0x5A, 7
    BSF 0x74, 7
    RETURN
__cmp_f32:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    ANDLW 0x7F
    SUBLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp73
    BTFSS 0x53, 7
    GOTO tmp73
    MOVF 0x53, W
    ANDLW 0x7F
    IORWF 0x52, W
    IORWF 0x51, W
    BTFSS STATUS, 2
    GOTO tmp75
tmp73:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x58, W
    ANDLW 0x7F
    SUBLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp74
    BTFSS 0x57, 7
    GOTO tmp74
    MOVF 0x57, W
    ANDLW 0x7F
    IORWF 0x56, W
    IORWF 0x55, W
    BTFSS STATUS, 2
    GOTO tmp75
tmp74:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp76
    BTFSC 0x53, 7
    GOTO tmp76
    MOVF 0x58, W
    ANDLW 0x7F
    BTFSS STATUS, 2
    GOTO tmp76
    BTFSC 0x57, 7
    GOTO tmp76
    GOTO tmp77
tmp76:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x54, W
    XORWF 0x58, W
    ANDLW 0x80
    BTFSS STATUS, 2
    GOTO tmp78
    MOVF 0x54, W
    ANDLW 0x80
    MOVWF 0x5A
    BCF 0x54, 7
    BCF 0x58, 7
    MOVF 0x51, W
    XORWF 0x55, W
    MOVWF 0x59
    MOVF 0x52, W
    XORWF 0x56, W
    IORWF 0x59, W
    MOVWF 0x59
    MOVF 0x53, W
    XORWF 0x57, W
    IORWF 0x59, W
    MOVWF 0x59
    MOVF 0x54, W
    XORWF 0x58, W
    IORWF 0x59, W
    MOVWF 0x59
    MOVF 0x55, W
    SUBWF 0x51, W
    MOVF 0x56, W
    BTFSS STATUS, 0
    INCFSZ 0x56, W
    SUBWF 0x52, W
    MOVF 0x57, W
    BTFSS STATUS, 0
    INCFSZ 0x57, W
    SUBWF 0x53, W
    MOVF 0x58, W
    BTFSS STATUS, 0
    INCFSZ 0x58, W
    SUBWF 0x54, W
    MOVF 0x59, W
    BTFSC STATUS, 2
    GOTO tmp77
    BTFSS STATUS, 0
    GOTO tmp81
    BTFSS 0x5A, 7
    GOTO tmp80
    GOTO tmp79
tmp81:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x5A, 7
    GOTO tmp79
    GOTO tmp80
tmp78:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x54, 7
    GOTO tmp80
    GOTO tmp79
tmp77:
    CLRF 0x71
    RETURN
tmp79:
    MOVLW 0x01
    MOVWF 0x71
    RETURN
tmp80:
    MOVLW 0x02
    MOVWF 0x71
    RETURN
tmp75:
    MOVLW 0x03
    MOVWF 0x71
    RETURN
    end
