volatile unsigned char in0;
volatile unsigned char out;
void main(void) {
  unsigned char t = 162u - in0;
  out = t;
  if (200u > in0) {
    out = (unsigned char)(out + 1);
  }
}
