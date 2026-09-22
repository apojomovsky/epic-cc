static _Bool flag = 1;
volatile unsigned char out = 0;
void set(void) { flag = 1; }
void clear(void) { flag = 0; }
int main(void) {
    if (flag) { out = 1; } else { out = 2; }
    set();
    clear();
    if (flag) { out = 3; }
    return 0;
}
