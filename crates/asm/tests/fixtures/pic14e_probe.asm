; pic14e P1 probe (DS41364E Table 29-3): every new instruction group the
; asm encoder must produce byte-for-byte, cross-checked against
; `gpasm -p 16f1937` 1.5.2 (2026-09-06). The classic opcodes are already
; covered by the PIC14 gpasm tests. Destination defaults (d = 1 when
; omitted, gpasm/MPASM documented), CLRW, RESET and the negative indexed
; forms are probed here too.
    list p=16f1937
    radix hex
FSR0 equ 0x004
FSR1 equ 0x006
    org 0
    OPTION
    TRIS 5
    TRIS 6
    TRIS 7
    MOVLB 0x1F
    MOVLP 0x7F
    ASRF 0x20, F
    LSLF 0x21, W
    LSRF 0x22, F
    ADDWFC 0x23, W
    SUBWFB 0x24, F
    ASRF 0x25        ; omitted destination defaults to d = 1
    LSLF 0x26
    DECF 0x27
    BRA -2
    BRW
    CALLW
    ADDFSR FSR0, -1
    ADDFSR FSR1, 0x1F
    MOVIW FSR0++
    MOVIW 3[FSR1]
    MOVWI --FSR0
    MOVWI 5[FSR1]
    MOVIW -3[FSR1]
    MOVWI -5[FSR0]
    CLRW
    RESET
    end
