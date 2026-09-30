/* Wide const-LHS subtraction (epic-cc#837): `K - x` over 16 bits lowers
 * through the borrow chain that parks C0 in the PIC18 flag bit, the one
 * fixed-region use no call or return accounts for. */
volatile unsigned short in;
volatile unsigned short out;
void main(void) { out = (unsigned short)(1000u - in); }
