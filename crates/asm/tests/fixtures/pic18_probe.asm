    list p=18f4550
    radix hex
    org 0x0000
    goto main
main:
    movlw 0x07
    movwf 0x20,A
    movlw 0x09
    addwf 0x20,W,A
    movwf 0x21,A
    clrf 0x22,A
    bsf 0x22,0,A
    btfsc 0x22,0,A
    bra skip
    bsf 0x22,1,A
skip:
    movlb 1
    movff 0x21,0x100
    lfsr 0,0x100
    movf 0x21,W,A
    call sub
    goto done
sub:
    incf 0x21,F,A
    return
done:
    movwf 0x23,A
    end
