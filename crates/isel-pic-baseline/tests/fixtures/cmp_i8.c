volatile unsigned char in0;
volatile unsigned char out;
void main(void) {
  if (in0 < 200u) {
    out = 1;
  } else {
    out = 2;
  }
}
