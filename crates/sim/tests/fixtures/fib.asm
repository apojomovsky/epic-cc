    list p=16f877a
    radix hex
    org 0
    goto start
start:
    movf 0x20, W
    addwf 0x21, W
    movwf 0x22
    sleep
    end
