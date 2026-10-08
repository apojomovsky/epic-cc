    list p=p18f4550
    radix hex

; call-result traffic through the ADR-013 retval region
call_chain:
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVWF 0x000,A ; slot byte into retval
    MOVWF 0x001,A ; slot byte into retval
    MOVWF 0x002,A ; slot byte into retval
    MOVWF 0x003,A ; slot byte into retval
    MOVFF 0x100, 0x000 ; slot to retval
    MOVFF 0x101, 0x001 ; slot to retval
    MOVFF 0x102, 0x002 ; slot to retval
    MOVFF 0x103, 0x003 ; slot to retval
    MOVFF 0x104, 0x000 ; slot to retval
    MOVFF 0x105, 0x001 ; slot to retval
    MOVFF 0x106, 0x002 ; slot to retval
    MOVFF 0x107, 0x003 ; slot to retval
    MOVFF 0x108, 0x000 ; slot to retval
    MOVFF 0x109, 0x001 ; slot to retval
    MOVFF 0x10A, 0x002 ; slot to retval
    MOVFF 0x10B, 0x003 ; slot to retval
    MOVFF 0x10C, 0x000 ; slot to retval
    MOVFF 0x10D, 0x001 ; slot to retval
    MOVFF 0x10E, 0x002 ; slot to retval
    MOVFF 0x10F, 0x003 ; slot to retval
    MOVFF 0x110, 0x000 ; slot to retval
    MOVFF 0x111, 0x001 ; slot to retval
    MOVFF 0x112, 0x002 ; slot to retval
    MOVFF 0x113, 0x003 ; slot to retval
    MOVFF 0x114, 0x000 ; slot to retval
    MOVFF 0x115, 0x001 ; slot to retval
    MOVFF 0x116, 0x002 ; slot to retval
    MOVFF 0x117, 0x003 ; slot to retval
    MOVFF 0x118, 0x000 ; slot to retval
    MOVFF 0x119, 0x001 ; slot to retval
    MOVFF 0x11A, 0x002 ; slot to retval
    MOVFF 0x11B, 0x003 ; slot to retval
    MOVFF 0x11C, 0x000 ; slot to retval
    MOVFF 0x11D, 0x001 ; slot to retval
    MOVFF 0x11E, 0x002 ; slot to retval
    MOVFF 0x11F, 0x003 ; slot to retval
    MOVFF 0x120, 0x000 ; slot to retval
    MOVFF 0x121, 0x001 ; slot to retval
    MOVFF 0x122, 0x002 ; slot to retval
    MOVFF 0x123, 0x003 ; slot to retval
    MOVFF 0x124, 0x000 ; slot to retval
    MOVFF 0x125, 0x001 ; slot to retval
    MOVFF 0x126, 0x002 ; slot to retval
    MOVFF 0x127, 0x003 ; slot to retval
    CLRF 0x000,A ; retval byte cleared
    CLRF 0x001,A ; retval byte cleared
    CLRF 0x002,A ; retval byte cleared
    CLRF 0x003,A ; retval byte cleared
    CLRF 0x000,A ; retval byte cleared
    CLRF 0x001,A ; retval byte cleared
    CLRF 0x002,A ; retval byte cleared
    CLRF 0x003,A ; retval byte cleared
    CLRF 0x000,A ; retval byte cleared
    CLRF 0x001,A ; retval byte cleared
    MOVWF 0x000,B
    MOVWF 0x001,B
    MOVWF 0x002,B
    MOVWF 0x003,B
    MOVWF 0x000,B
    MOVWF 0x001,B
    MOVWF 0x002,B
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVF 0x002,W,A ; retval byte back to W
    MOVF 0x003,W,A ; retval byte back to W
    MOVF 0x000,W,A ; retval byte back to W
    MOVF 0x001,W,A ; retval byte back to W
    MOVFF 0x000, 0x200 ; retval to home slot
    MOVFF 0x001, 0x201 ; retval to home slot
    MOVFF 0x002, 0x202 ; retval to home slot
    MOVFF 0x003, 0x203 ; retval to home slot
    MOVFF 0x000, 0x204 ; retval to home slot
    MOVFF 0x001, 0x205 ; retval to home slot
    MOVFF 0x002, 0x206 ; retval to home slot
    MOVFF 0x003, 0x207 ; retval to home slot
    MOVFF 0x000, 0x208 ; retval to home slot
    MOVFF 0x001, 0x209 ; retval to home slot
    MOVFF 0x002, 0x20A ; retval to home slot
    MOVFF 0x003, 0x20B ; retval to home slot
    MOVFF 0x000, 0x20C ; retval to home slot
    MOVFF 0x001, 0x20D ; retval to home slot
    MOVFF 0x002, 0x20E ; retval to home slot
    MOVFF 0x003, 0x20F ; retval to home slot
    MOVFF 0x000, 0x210 ; retval to home slot
    MOVFF 0x001, 0x211 ; retval to home slot
    MOVFF 0x002, 0x212 ; retval to home slot
    MOVFF 0x003, 0x213 ; retval to home slot
    MOVFF 0x000, 0x214 ; retval to home slot
    MOVFF 0x001, 0x215 ; retval to home slot
    MOVFF 0x002, 0x216 ; retval to home slot
    MOVFF 0x003, 0x217 ; retval to home slot
    MOVFF 0x000, 0x218 ; retval to home slot
    MOVFF 0x001, 0x219 ; retval to home slot
    MOVFF 0x002, 0x21A ; retval to home slot
    MOVFF 0x003, 0x21B ; retval to home slot
    MOVFF 0x000, 0x21C ; retval to home slot
    MOVFF 0x001, 0x21D ; retval to home slot
    MOVFF 0x002, 0x21E ; retval to home slot
    MOVFF 0x003, 0x21F ; retval to home slot
    MOVFF 0x000, 0x220 ; retval to home slot
    MOVFF 0x001, 0x221 ; retval to home slot
    MOVFF 0x002, 0x222 ; retval to home slot
    MOVFF 0x003, 0x223 ; retval to home slot
    MOVFF 0x000, 0x224 ; retval to home slot
    MOVFF 0x001, 0x225 ; retval to home slot
    MOVFF 0x002, 0x226 ; retval to home slot
    MOVFF 0x003, 0x227 ; retval to home slot
    MOVFF 0x000, 0x228 ; retval to home slot
    MOVFF 0x001, 0x229 ; retval to home slot
    MOVFF 0x002, 0x22A ; retval to home slot
    MOVFF 0x003, 0x22B ; retval to home slot
    MOVFF 0x000, 0x22C ; retval to home slot
    MOVFF 0x001, 0x22D ; retval to home slot
    MOVFF 0x002, 0x22E ; retval to home slot
    MOVFF 0x003, 0x22F ; retval to home slot
    MOVFF 0x000, 0x230 ; retval to home slot
    MOVFF 0x001, 0x231 ; retval to home slot
    MOVFF 0x002, 0x232 ; retval to home slot
    MOVFF 0x003, 0x233 ; retval to home slot
    MOVFF 0x000, 0x234 ; retval to home slot
    MOVFF 0x001, 0x235 ; retval to home slot
negatives:
    MOVWF 0x004,A ; ISR save area, past the retval region
    MOVF 0x008,W,A
    MOVFF 0x00C, 0x010
    MOVFF 0x010, 0x00F
    MOVWF retval_lo,A ; symbolic retval name is not an address
    MOVFF retval_lo, 0x010
    BSF 0x000,0,A ; bit op, not a data move
