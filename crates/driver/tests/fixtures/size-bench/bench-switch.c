volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    switch (in) {
    case 0: out = 10; break;
    case 1: out = 11; break;
    case 2: out = 12; break;
    case 3: out = 13; break;
    case 4: out = 14; break;
    case 5: out = 15; break;
    case 200: out = 16; break;
    default: out = 0; break;
    }
}
