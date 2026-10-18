    list p=16f877a
    radix hex
    org 0
; OPTION 0xD1: internal clock, prescaler on TMR0 at 1:4.
    movlw 0xD1
    option
loop:
    goto loop
    end
