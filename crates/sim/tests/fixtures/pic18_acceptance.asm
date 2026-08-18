    list p=18f4550
    radix hex
    org 0x0000
    goto main
main:
    movlw 0x05
    movwf 0x20,A
    movlw 0x07
    addwf 0x20,W,A
    movwf 0x21,A
loop:
    decfsz 0x21,F,A
    bra loop
    movff 0x20,0x23
    call double
    movwf 0x23,A
    movlb 1
    movwf 0x20,B
    bra finish

double:
    movf 0x23,W,A
    addwf 0x23,F,A
    movf 0x23,W,A
    return

finish:
    movwf 0x24,A
    sleep
    end
