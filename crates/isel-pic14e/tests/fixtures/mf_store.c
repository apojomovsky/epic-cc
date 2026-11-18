
volatile unsigned char flag; volatile unsigned char out; char buf[4]="abcd";
int main(void){ char *p = flag ? buf : "PQRS"; p[0]='Z'; out=(unsigned char)p[0]; return 0; }
