volatile unsigned char sink;
typedef struct { unsigned char b[16]; unsigned short tag; } Block;
typedef unsigned char (*probe_t)(const Block *);
volatile probe_t pfn;
static unsigned char probe(const Block *p) {
    return (unsigned char)(p->b[0] ^ p->b[7] ^ p->b[15] ^ (p->tag & 0xFF));
}
void main(void) {
    Block blk = { { 0x10,0x21,0x32,0x43,0x54,0x65,0x76,0x87,0x98,0xA9,0xBA,0xCB,0xDC,0xED,0xFE,0x0F }, 0x1234 };
    pfn = probe;
    sink = pfn(&blk);
}
