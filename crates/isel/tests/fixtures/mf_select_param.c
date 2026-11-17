
volatile unsigned char flag;
volatile unsigned char out;
char buf[4] = "abcd";
static void use(const char *p){ out = (unsigned char)p[1]; }
int main(void){ use(flag ? buf : "PQRS"); return 0; }
