    list p=p18f4550
    radix hex

; folded HAL init zero-fill, control-demo cluster excerpt
zero_0:
    CLRF 0x028,A
    CLRF 0x029,A
    CLRF 0x02A,A
    CLRF 0x02B,A
    CLRF 0x02C,A
    CLRF 0x02D,A
    CLRF 0x02E,A
    CLRF 0x02F,A
    CLRF 0x030,A
    CLRF 0x031,A
    CLRF 0x032,A
    CLRF 0x033,A
    CLRF 0x034,A
    CLRF 0x035,A
    CLRF 0x036,A
zero_1:
    CLRF 0x047,A
    CLRF 0x048,A
    CLRF 0x049,A
    CLRF 0x04A,A
    CLRF 0x04B,A
    CLRF 0x04C,A
    CLRF 0x04D,A
    CLRF 0x04E,A
zero_2:
    CLRF 0x05F,A
    CLRF 0x060,A
    CLRF 0x061,A
    CLRF 0x062,A
    CLRF 0x063,A
    CLRF 0x064,A
    CLRF 0x065,A
    CLRF 0x066,A
zero_3:
    CLRF 0x077,A
    CLRF 0x078,A
    CLRF 0x079,A
    CLRF 0x07A,A
    CLRF 0x07B,A
    CLRF 0x07C,A
    MOVWF 0x0FF,A ; nonzero store separates runs
zero_4:
    CLRF 0x08D,A
    CLRF 0x08E,A
    CLRF 0x08F,A
zero_5:
    CLRF 0x0A0,A
    CLRF 0x0A1,A
    CLRF 0x0A2,A
zero_6:
    CLRF 0x0B3,A
    CLRF 0x0B4,A
    CLRF 0x0B5,A
zero_7:
    CLRF 0x0C6,A
    CLRF 0x0C7,A
    CLRF 0x0C8,A
zero_8:
    CLRF 0x0D9,A
    CLRF 0x0DA,A
    CLRF 0x0DB,A
zero_9:
    CLRF 0x0EC,A
    CLRF 0x0ED,A
    CLRF 0x0EE,A
    MOVWF 0x0FF,A ; nonzero store separates runs
zero_10:
    CLRF 0x0FF,A
    CLRF 0x100,A
zero_11:
    CLRF 0x111,A
    CLRF 0x112,A
zero_12:
    CLRF 0x123,A
    CLRF 0x124,A
zero_13:
    CLRF 0x135,A
    CLRF 0x136,A
    CLRF 0x137,A
    CLRF 0x138,A
zero_14:
    CLRF 0x149,A
    CLRF 0x14A,A
    CLRF 0x14B,A
    CLRF 0x14C,A
    CLRF 0x14D,A
zero_15:
    CLRF 0x15E,A
    CLRF 0x15F,A
    CLRF 0x160,A
    CLRF 0x161,A
    CLRF 0x162,A
    CLRF 0x163,A
    CLRF 0x164,A
zero_16:
    CLRF 0x175,A
    CLRF 0x176,A
    CLRF 0x177,A
    CLRF 0x178,A
    CLRF 0x179,A
    CLRF 0x17A,A
    CLRF 0x17B,A
    CLRF 0x17C,A
    CLRF 0x17D,A
negatives:
    CLRF 0x100,A ; isolated singles are inline stores, not runs
    MOVWF 0x100,A
    clrf 0x101,a
    MOVF 0x101,W,A
    CLRF 0x102,A
