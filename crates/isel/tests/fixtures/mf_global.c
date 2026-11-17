
volatile unsigned char flag;
volatile unsigned char out;
char buf[4] = "abcd";
const char *g;
int main(void){ g = "PQRS"; if (flag) g = buf; out = (unsigned char)g[1]; return 0; }
