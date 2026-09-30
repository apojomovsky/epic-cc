unsigned char g_log[4];
unsigned char g_n;

void note(unsigned char v) {
    if (g_n < 4) {
        g_log[g_n] = v;
        g_n = g_n + 1;
    }
}

unsigned char out_vals[3];
unsigned char out_n;

int main(void) {
    unsigned char x;
    for (x = 0; x < 3; x++) {
        if (x == 0) {
            out_vals[x] = 10;
            note(1);
        } else if (x == 1) {
            out_vals[x] = 20;
            note(2);
            note(1);
        } else {
            out_vals[x] = 30;
            note(1);
        }
    }
    out_n = g_n;
    return 0;
}
