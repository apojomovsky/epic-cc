//! epic-cc#443 repro: a `static const` struct whose pointer field is
//! initialized with the ADDRESS of a RAM global (the register-map shape
//! epic-hal's combo-modbus-full uses). The const table's `refs` name a
//! RAM global, which has no assembler label: the materialized bytes must
//! be the alloc-time RAM address, not a `LOW()`/`HIGH()` label literal.
//! The volatile view is what keeps the refs in the IR: a volatile
//! pointer can hold anything at runtime, so neither clang nor the
//! whole-program opt may fold the deref chain away.
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
