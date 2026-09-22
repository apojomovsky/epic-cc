// epic-cc#452: a `static const` struct whose pointer field holds a RAM
// global's ADDRESS (the register-map shape). The const table's refs name
// a RAM global, which has no assembler label: the materialized bytes must
// be the alloc-time RAM address, not LOW()/HIGH() label literals. The
// volatile view keeps the refs alive through optimization.
// No stdint: the harness compiles with -nostdinc.
static unsigned short holding_regs[4];

struct reg_map {
    const unsigned short *regs;
    unsigned char count;
};

static const struct reg_map map = { holding_regs, 4 };
static const struct reg_map *volatile s_map = &map;

static volatile unsigned char out;

void main(void) {
    const struct reg_map *m = s_map;
    out = (unsigned char)(m->regs[1] + m->count);
}
