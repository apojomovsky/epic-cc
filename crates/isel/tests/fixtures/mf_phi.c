
volatile unsigned char flag;
volatile unsigned char out;
char buf[4] = "abcd";
int main(void){ const char *p = "PQRS"; if (flag) p = buf; out = (unsigned char)p[1]; return 0; }
