volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    out = (unsigned char)(in > 3u);
    out = (unsigned char)((in == 7u) + out);
}
