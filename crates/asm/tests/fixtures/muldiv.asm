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
    MOVWF 0x25
    MOVF 0x21, W
    MOVWF 0x26
    MOVF 0x25, W
    MOVWF 0x5D
    MOVF 0x26, W
    MOVWF 0x5E
    MOVLW 0x07
    MOVWF 0x5F
    MOVLW 0x00
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __udiv_u16
    MOVF 0x71, W
    MOVWF 0x27
    MOVF 0x72, W
    MOVWF 0x28
    MOVF 0x27, W
    MOVWF 0x22
    MOVF 0x28, W
    MOVWF 0x23
    MOVF 0x22, W
    MOVWF 0x29
    MOVF 0x23, W
    MOVWF 0x2A
    MOVF 0x29, W
    MOVWF 0x5D
    MOVF 0x2A, W
    MOVWF 0x5E
    MOVLW 0x03
    MOVWF 0x5F
    MOVLW 0x00
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __mul_u16
    MOVF 0x71, W
    MOVWF 0x2B
    MOVF 0x72, W
    MOVWF 0x2C
    MOVF 0x25, W
    MOVWF 0x5D
    MOVF 0x26, W
    MOVWF 0x5E
    MOVLW 0x05
    MOVWF 0x5F
    MOVLW 0x00
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __urem_u16
    MOVF 0x71, W
    MOVWF 0x2D
    MOVF 0x72, W
    MOVWF 0x2E
    MOVF 0x2D, W
    ADDWF 0x2B, W
    MOVWF 0x2F
    MOVF 0x2E, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x2C, W
    MOVWF 0x30
    MOVF 0x2F, W
    MOVWF 0x22
    MOVF 0x30, W
    MOVWF 0x23
    MOVF 0x22, W
    MOVWF 0x31
    MOVF 0x23, W
    MOVWF 0x32
    MOVF 0x31, W
    MOVWF 0x33
    MOVF 0x32, W
    MOVWF 0x34
    BCF STATUS, 0
    RLF 0x33, F
    RLF 0x34, F
    BCF STATUS, 0
    RLF 0x33, F
    RLF 0x34, F
    MOVF 0x33, W
    MOVWF 0x22
    MOVF 0x34, W
    MOVWF 0x23
    MOVF 0x22, W
    MOVWF 0x35
    MOVF 0x23, W
    MOVWF 0x36
    MOVF 0x35, W
    MOVWF 0x37
    MOVF 0x36, W
    MOVWF 0x38
    BCF STATUS, 0
    RRF 0x38, F
    RRF 0x37, F
    BCF STATUS, 0
    RRF 0x38, F
    RRF 0x37, F
    BCF STATUS, 0
    RRF 0x38, F
    RRF 0x37, F
    MOVF 0x25, W
    MOVWF 0x39
    MOVF 0x26, W
    MOVWF 0x3A
    BCF STATUS, 0
    RRF 0x3A, F
    RRF 0x39, F
    BCF STATUS, 0
    RRF 0x3A, F
    RRF 0x39, F
    BCF STATUS, 0
    RRF 0x3A, F
    RRF 0x39, F
    BCF STATUS, 0
    RRF 0x3A, F
    RRF 0x39, F
    MOVF 0x39, W
    IORWF 0x37, W
    MOVWF 0x3B
    MOVF 0x3A, W
    IORWF 0x38, W
    MOVWF 0x3C
    MOVF 0x3B, W
    MOVWF 0x22
    MOVF 0x3C, W
    MOVWF 0x23
    MOVF 0x25, W
    ADDLW 0xC0
    MOVWF 0x3D
    MOVF 0x26, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDLW 0xFE
    MOVWF 0x3E
    MOVF 0x3D, W
    MOVWF 0x5D
    MOVF 0x3E, W
    MOVWF 0x5E
    MOVLW 0xFD
    MOVWF 0x5F
    MOVLW 0xFF
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __sdiv_i16
    MOVF 0x71, W
    MOVWF 0x3F
    MOVF 0x72, W
    MOVWF 0x40
    MOVF 0x3F, W
    MOVWF 0x22
    MOVF 0x40, W
    MOVWF 0x23
    MOVF 0x3D, W
    MOVWF 0x5D
    MOVF 0x3E, W
    MOVWF 0x5E
    MOVLW 0x03
    MOVWF 0x5F
    MOVLW 0x00
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __srem_i16
    MOVF 0x71, W
    MOVWF 0x41
    MOVF 0x72, W
    MOVWF 0x42
    MOVF 0x22, W
    MOVWF 0x43
    MOVF 0x23, W
    MOVWF 0x44
    MOVF 0x41, W
    ADDWF 0x43, W
    MOVWF 0x45
    MOVF 0x42, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x44, W
    MOVWF 0x46
    MOVF 0x45, W
    MOVWF 0x22
    MOVF 0x46, W
    MOVWF 0x23
    MOVF 0x25, W
    MOVWF 0x47
    MOVF 0x47, W
    MOVWF 0x24
    MOVF 0x24, W
    MOVWF 0x48
    MOVF 0x48, W
    MOVWF 0x49
    CLRF 0x4A
    MOVF 0x48, W
    MOVWF 0x5D
    MOVLW 0x07
    MOVWF 0x5E
    MOVLW 0x00
    MOVWF PCLATH
    CALL __mul_u8
    MOVF 0x71, W
    MOVWF 0x4B
    MOVF 0x4B, W
    MOVWF 0x5D
    MOVLW 0x03
    MOVWF 0x5E
    MOVLW 0x00
    MOVWF PCLATH
    CALL __udiv_u8
    MOVF 0x71, W
    MOVWF 0x4C
    MOVF 0x22, W
    MOVWF 0x4D
    MOVF 0x23, W
    MOVWF 0x4E
    MOVF 0x4C, W
    MOVWF 0x4F
    CLRF 0x50
    MOVF 0x4F, W
    ADDWF 0x4D, W
    MOVWF 0x51
    MOVF 0x50, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x4E, W
    MOVWF 0x52
    MOVF 0x51, W
    MOVWF 0x5D
    MOVF 0x52, W
    MOVWF 0x5E
    MOVLW 0x05
    MOVWF 0x5F
    MOVLW 0x00
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __mul_u16
    MOVF 0x71, W
    MOVWF 0x53
    MOVF 0x72, W
    MOVWF 0x54
    MOVF 0x53, W
    MOVWF 0x22
    MOVF 0x54, W
    MOVWF 0x23
    MOVF 0x22, W
    MOVWF 0x55
    MOVF 0x23, W
    MOVWF 0x56
    MOVF 0x25, W
    ANDLW 0x03
    MOVWF 0x57
    MOVF 0x26, W
    ANDLW 0x00
    MOVWF 0x58
    MOVF 0x49, W
    MOVWF 0x5D
    MOVF 0x4A, W
    MOVWF 0x5E
    MOVF 0x57, W
    MOVWF 0x5F
    MOVF 0x58, W
    MOVWF 0x60
    MOVLW 0x00
    MOVWF PCLATH
    CALL __shl_u16
    MOVF 0x71, W
    MOVWF 0x59
    MOVF 0x72, W
    MOVWF 0x5A
    MOVF 0x59, W
    ADDWF 0x55, W
    MOVWF 0x5B
    MOVF 0x5A, W
    BTFSC STATUS, 0 ; C
    ADDLW 0x01
    ADDWF 0x56, W
    MOVWF 0x5C
    MOVF 0x5B, W
    MOVWF 0x22
    MOVF 0x5C, W
    MOVWF 0x23
    RETURN

__udiv_u16:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x61
    CLRF 0x62
    MOVLW 0x10
    MOVWF 0x63
tmp0:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5E, F
    RLF 0x61, F
    RLF 0x62, F
    MOVF 0x5F, W
    SUBWF 0x61, F
    MOVF 0x60, W
    BTFSS STATUS, 0
    INCFSZ 0x60, W
    SUBWF 0x62, F
    BTFSS STATUS, 0
    GOTO tmp1
    BSF 0x5D, 0
    GOTO tmp2
tmp1:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5F, W
    ADDWF 0x61, F
    MOVF 0x60, W
    BTFSC STATUS, 0
    INCFSZ 0x60, W
    ADDWF 0x62, F
tmp2:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x63, F
    GOTO tmp0
    MOVF 0x5D, W
    MOVWF 0x71
    MOVF 0x5E, W
    MOVWF 0x72
    RETURN
__mul_u16:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x64
    CLRF 0x65
    CLRF 0x66
    CLRF 0x67
    CLRF 0x68
    CLRF 0x69
    CLRF 0x6A
    CLRF 0x6B
    MOVF 0x5D, W
    MOVWF 0x68
    MOVF 0x5E, W
    MOVWF 0x69
    MOVF 0x5F, W
    MOVWF 0x61
    MOVF 0x60, W
    MOVWF 0x62
    MOVLW 0x10
    MOVWF 0x63
tmp3:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x61, 0
    GOTO tmp4
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
    MOVF 0x6B, W
    BTFSC STATUS, 0
    INCFSZ 0x6B, W
    ADDWF 0x67, F
tmp4:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x68, F
    RLF 0x69, F
    RLF 0x6A, F
    RLF 0x6B, F
    BCF STATUS, 0
    RRF 0x62, F
    RRF 0x61, F
    DECFSZ 0x63, F
    GOTO tmp3
    MOVF 0x64, W
    MOVWF 0x71
    MOVF 0x65, W
    MOVWF 0x72
    RETURN
__urem_u16:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x61
    CLRF 0x62
    MOVLW 0x10
    MOVWF 0x63
tmp5:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5E, F
    RLF 0x61, F
    RLF 0x62, F
    MOVF 0x5F, W
    SUBWF 0x61, F
    MOVF 0x60, W
    BTFSS STATUS, 0
    INCFSZ 0x60, W
    SUBWF 0x62, F
    BTFSS STATUS, 0
    GOTO tmp6
    BSF 0x5D, 0
    GOTO tmp7
tmp6:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5F, W
    ADDWF 0x61, F
    MOVF 0x60, W
    BTFSC STATUS, 0
    INCFSZ 0x60, W
    ADDWF 0x62, F
tmp7:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x63, F
    GOTO tmp5
    MOVF 0x61, W
    MOVWF 0x71
    MOVF 0x62, W
    MOVWF 0x72
    RETURN
__sdiv_i16:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x61
    BTFSS 0x5E, 7
    GOTO tmp8
    BSF 0x61, 1
    BSF 0x61, 0
    COMF 0x5D, F
    COMF 0x5E, F
    INCF 0x5D, F
    BTFSC STATUS, 2
    INCF 0x5E, F
tmp8:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x60, 7
    GOTO tmp9
    COMF 0x5F, F
    COMF 0x60, F
    INCF 0x5F, F
    BTFSC STATUS, 2
    INCF 0x60, F
    MOVLW 0x01
    XORWF 0x61, F
tmp9:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x62
    CLRF 0x63
    MOVLW 0x10
    MOVWF 0x64
tmp10:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5E, F
    RLF 0x62, F
    RLF 0x63, F
    MOVF 0x5F, W
    SUBWF 0x62, F
    MOVF 0x60, W
    BTFSS STATUS, 0
    INCFSZ 0x60, W
    SUBWF 0x63, F
    BTFSS STATUS, 0
    GOTO tmp11
    BSF 0x5D, 0
    GOTO tmp12
tmp11:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5F, W
    ADDWF 0x62, F
    MOVF 0x60, W
    BTFSC STATUS, 0
    INCFSZ 0x60, W
    ADDWF 0x63, F
tmp12:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x64, F
    GOTO tmp10
    BTFSS 0x61, 0
    GOTO tmp13
    COMF 0x5D, F
    COMF 0x5E, F
    INCF 0x5D, F
    BTFSC STATUS, 2
    INCF 0x5E, F
tmp13:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5D, W
    MOVWF 0x71
    MOVF 0x5E, W
    MOVWF 0x72
    RETURN
__srem_i16:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x61
    BTFSS 0x5E, 7
    GOTO tmp14
    BSF 0x61, 1
    BSF 0x61, 0
    COMF 0x5D, F
    COMF 0x5E, F
    INCF 0x5D, F
    BTFSC STATUS, 2
    INCF 0x5E, F
tmp14:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x60, 7
    GOTO tmp15
    COMF 0x5F, F
    COMF 0x60, F
    INCF 0x5F, F
    BTFSC STATUS, 2
    INCF 0x60, F
    MOVLW 0x01
    XORWF 0x61, F
tmp15:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x62
    CLRF 0x63
    MOVLW 0x10
    MOVWF 0x64
tmp16:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5E, F
    RLF 0x62, F
    RLF 0x63, F
    MOVF 0x5F, W
    SUBWF 0x62, F
    MOVF 0x60, W
    BTFSS STATUS, 0
    INCFSZ 0x60, W
    SUBWF 0x63, F
    BTFSS STATUS, 0
    GOTO tmp17
    BSF 0x5D, 0
    GOTO tmp18
tmp17:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5F, W
    ADDWF 0x62, F
    MOVF 0x60, W
    BTFSC STATUS, 0
    INCFSZ 0x60, W
    ADDWF 0x63, F
tmp18:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x64, F
    GOTO tmp16
    BTFSS 0x61, 1
    GOTO tmp19
    COMF 0x62, F
    COMF 0x63, F
    INCF 0x62, F
    BTFSC STATUS, 2
    INCF 0x63, F
tmp19:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x62, W
    MOVWF 0x71
    MOVF 0x63, W
    MOVWF 0x72
    RETURN
__mul_u8:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x61
    CLRF 0x62
    CLRF 0x63
    CLRF 0x64
    MOVF 0x5D, W
    MOVWF 0x63
    MOVF 0x5E, W
    MOVWF 0x5F
    MOVLW 0x08
    MOVWF 0x60
tmp20:
    BCF STATUS, 5
    BCF STATUS, 6
    BTFSS 0x5F, 0
    GOTO tmp21
    MOVF 0x63, W
    ADDWF 0x61, F
    MOVF 0x64, W
    BTFSC STATUS, 0
    INCFSZ 0x64, W
    ADDWF 0x62, F
tmp21:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x63, F
    RLF 0x64, F
    BCF STATUS, 0
    RRF 0x5F, F
    DECFSZ 0x60, F
    GOTO tmp20
    MOVF 0x61, W
    MOVWF 0x71
    MOVF 0x62, W
    MOVWF 0x72
    RETURN
__udiv_u8:
    BCF STATUS, 5
    BCF STATUS, 6
    CLRF 0x5F
    CLRF 0x60
    MOVLW 0x08
    MOVWF 0x61
tmp22:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5F, F
    RLF 0x60, F
    MOVF 0x5E, W
    SUBWF 0x5F, F
    MOVLW 0x00
    BTFSS STATUS, 0
    ADDLW 0x01
    SUBWF 0x60, F
    BTFSS STATUS, 0
    GOTO tmp23
    BSF 0x5D, 0
    GOTO tmp24
tmp23:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5E, W
    ADDWF 0x5F, F
    MOVLW 0x00
    BTFSC STATUS, 0
    ADDLW 0x01
    ADDWF 0x60, F
tmp24:
    BCF STATUS, 5
    BCF STATUS, 6
    DECFSZ 0x61, F
    GOTO tmp22
    MOVF 0x5D, W
    MOVWF 0x71
    RETURN
__shl_u16:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5F, W
    ANDLW 0x0F
    MOVWF 0x61
    CLRF 0x62
    MOVF 0x61, F
    BTFSC STATUS, 2
    GOTO tmp26
tmp25:
    BCF STATUS, 0
    BCF STATUS, 5
    BCF STATUS, 6
    RLF 0x5D, F
    RLF 0x5E, F
    DECFSZ 0x61, F
    GOTO tmp25
tmp26:
    BCF STATUS, 5
    BCF STATUS, 6
    MOVF 0x5D, W
    MOVWF 0x71
    MOVF 0x5E, W
    MOVWF 0x72
    RETURN
    end
