//! epic-cc#778 acceptance: a run of same-table dynamically indexed const
//! reads sharing one TBLPTR seed.
//!
//! Four consecutive bytes of one const struct at a runtime index
//! (`SCRIPT[i].tick` + `SCRIPT[i].event`): the index term is identical for
//! all four sites and the static offsets step 0-3, so the backend seeds
//! once and walks `TBLRD*+` instead of reseeding per site. `g_idx` is
//! written by the test harness, never by the program, so every index
//! stays dynamic exactly like the menu-demo stimulus shape.
typedef struct {
    unsigned int tick;
    unsigned int event;
} stimulus_t;

static const stimulus_t SCRIPT[] = {
    { 5U, 1U },
    { 10U, 2U },
    { 15U, 3U },
    { 20U, 4U },
    { 25U, 5U },
};

unsigned char g_idx;
unsigned int g_tick;
unsigned int g_event;

int main(void) {
    unsigned char i = g_idx;
    g_tick = SCRIPT[i].tick;
    g_event = SCRIPT[i].event;
    return 0;
}
