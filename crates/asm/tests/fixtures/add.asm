    list p=16f877a
    radix hex
    org 0x0000
    goto __start
main:
    MOVF 0x20, W
    ADDLW 0x01
    MOVWF 0x21
    RETURN
__start:
    CALL main
    SLEEP
    end
