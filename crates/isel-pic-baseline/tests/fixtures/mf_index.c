
volatile unsigned char flag; volatile unsigned char out; volatile unsigned char idx; char buf[4]="abcd";
int main(void){ const char *p = flag ? buf : "PQRS"; out=(unsigned char)p[idx]; return 0; }
