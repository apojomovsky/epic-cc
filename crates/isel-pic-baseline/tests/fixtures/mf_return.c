
volatile unsigned char flag;
volatile unsigned char out;
char buf[4] = "abcd";
static const char *get(void){ return flag ? buf : "PQRS"; }
int main(void){ const char *p = get(); out = (unsigned char)p[1]; return 0; }
