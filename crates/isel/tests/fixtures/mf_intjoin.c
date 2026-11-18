volatile unsigned char flag;
volatile unsigned char out;
char buf[4] = "abcd";
const char *g;
int main(void){
    g = "PQRS";
    unsigned short x = (unsigned short)g;
    const char *p = (const char *)x;
    const char *q = flag ? buf : p;
    out = (unsigned char)q[1];
    return 0;
}
