
volatile unsigned char flag; volatile unsigned char out; unsigned short buf[2]={0x4241,0x4443}; const unsigned short tab[2]={0x5150,0x5352};
int main(void){ const unsigned short *p = flag ? buf : tab; out=(unsigned char)(p[1]&0xFF); return 0; }
