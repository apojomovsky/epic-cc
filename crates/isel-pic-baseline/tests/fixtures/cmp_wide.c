// P2 wide-compare regression (epic-cc#383): the high-byte borrow fold
// must survive a 0xFF subtrahend with borrow pending. Each compare lands
// in its own out byte; the two seed sets cover the wrap (A) and the
// adjacent folds (B: signed 0x7F-high wrap, borrow without wrap).
// A: sa = 0x007F, sb = 0xFF80, ua = 0x007F, ub = 0xFF80 -> [1,1,0,0,0,1].
// B: sa = 0x7F7F, sb = 0x7F80, ua = 0x00FF, ub = 0x0100 -> [0,1,0,0,1,1].
volatile short sa;
volatile short sb;
volatile unsigned short ua;
volatile unsigned short ub;
volatile unsigned short o1;
volatile unsigned short o2;
volatile unsigned short o3;
volatile unsigned short o4;
volatile unsigned short o5;
volatile unsigned short o6;
void main(void) {
    o1 = (unsigned short)(sa > sb);
    o2 = (unsigned short)(sa > -128);
    o3 = (unsigned short)(ua > ub);
    o4 = (unsigned short)(ua > 0xFF80u);
    o5 = (unsigned short)(sb > sa);
    o6 = (unsigned short)(ub > ua);
}
