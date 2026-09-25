// Entry TU for the gate-A reads: fetches every row through the
// `extern` declaration with runtime-unknown indices (read off the
// volatile seed, so the reads stay live to isel) and folds the fields
// into the volatile sink. `% 5` keeps every possible index in bounds.
// At power-on the seed is 0 and the indices are 0..4. Expected:
// flags 11+22+33+44+55 = 165, values 101+202+303+404+505 = 1515,
// sink = ((165 + 1515) & 0xFF) ^ 0xA5 = 0x35, halted.

struct Item {
    unsigned char flags;
    unsigned int value;
};

unsigned char get_flag(unsigned char irq);
unsigned int get_value(unsigned char irq);
volatile unsigned char g_seed;
volatile unsigned short g_sum;
volatile unsigned char g_done;

void main(void)
{
    unsigned short facc = 0u;
    unsigned short vacc = 0u;
    unsigned char base = (unsigned char)(g_seed & 7u);
    for (unsigned char k = 0u; k < 5u; k++)
    {
        unsigned char irq = (unsigned char)((k + base) % 5u);
        facc = (unsigned short)(facc + get_flag(irq));
        vacc = (unsigned short)(vacc + get_value(irq));
    }
    g_sum = (unsigned short)(facc + vacc);
    g_done = (unsigned char)(((facc + vacc) & 0xFFu) ^ 0xA5u);
}
