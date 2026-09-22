//! epic-cc#451 acceptance for the enhanced mid-range: a `static const`
//! struct whose pointer field holds a RAM global's ADDRESS (epic-hal's
//! register-map shape) compiles through the whole pipeline to a 16F1937
//! HEX. The const table's refs name a RAM global, which has no assembler
//! label: the emitted bytes are the alloc-time address, not a
//! `LOW()`/`HIGH()` label literal.
#include <stdint.h>

static uint16_t holding_regs[4];

struct reg_map {
    const uint16_t *regs;
    uint8_t count;
};

static const struct reg_map map = { holding_regs, 4 };
static const struct reg_map *volatile s_map = &map;

static uint8_t out;

int main(void) {
    holding_regs[0] = 0x5Au;
    out = (uint8_t)(s_map->regs[0] == 0x5Au);
    return 0;
}
