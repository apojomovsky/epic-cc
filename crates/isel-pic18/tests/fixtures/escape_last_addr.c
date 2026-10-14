unsigned char buf[8] = {1, 2, 3, 4, 5, 6, 7, 8};
unsigned int n = 2;
unsigned char sum;
unsigned char *last;

void main(void) {
    unsigned int i;
    unsigned char *p;
    for (i = 0; i < n; i++) {
        p = &buf[i];
        sum += *p;
    }
    last = p;
}
