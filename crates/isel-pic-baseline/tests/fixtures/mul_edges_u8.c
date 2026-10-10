// Baseline __mul_u8 edge cases: zero operand exits early, product wraps mod 256.
volatile unsigned char pa8[2];
volatile unsigned char pb8[2];

void main(void) {
    for (unsigned char i = 0; i < 2; i++) {
        pa8[i] = (unsigned char)(pa8[i] * pb8[i]);
    }
}
