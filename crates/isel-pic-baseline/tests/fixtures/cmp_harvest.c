// Minimal source for harvesting real clang -O1 IR (epic-cc#383).
volatile short sb;
volatile unsigned short ub;
volatile unsigned short c1;
volatile unsigned short c2;
volatile unsigned short c3;
volatile unsigned short c4;
void main(void) {
    c1 = (unsigned short)(ub > 0x0100u);
    c2 = (unsigned short)(ub < 0x0080u);
    c3 = (unsigned short)(sb > 0x7F);
    c4 = (unsigned short)(sb > 0x7F00);
}
