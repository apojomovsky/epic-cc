
volatile unsigned char flag; volatile unsigned char out; char buf[4]="abcd"; char dst[4];
int main(void){ const char *p = flag ? buf : "PQRS"; __builtin_memcpy(dst,p,3); out=(unsigned char)dst[1]; return 0; }
