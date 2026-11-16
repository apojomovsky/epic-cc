/* epic-cc#1487: shared shapes for the three gep_overflow_1487 units.
   The rx field sits at offset 1, so a pointer to it is itself a GEP
   over the struct global: the base every decode access chains from. */
struct framer {
    unsigned char link;
    unsigned char rx[16];
    unsigned short len;
};
unsigned short port_read(unsigned char *buf, unsigned short max);
unsigned char decode(unsigned char *buf, unsigned short len, unsigned short *out_len);
unsigned char poll(struct framer *f, unsigned short max);
