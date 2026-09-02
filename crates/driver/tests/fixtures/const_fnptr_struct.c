// epic-cc#154 acceptance: a `static const` struct with a function-pointer
// field (a table-driven FSM's transition rows) must decode and dispatch.
//
// `table` is a const struct array whose `guard` field holds a function
// address; irparse decodes the table with the fn-ptr field as a link-time
// label reference, isel emits LOW/HIGH label literals in the RETLW table,
// and the indirect call through the loaded field dispatches the candidate.
//
// Expected (sim sets `g_idx` before run):
//   - g_idx = 0: row 0's guard is non-null -> out = guard(0) = 1
//     (g_count < 2)
//   - g_idx = 1: row 1's guard is null -> out stays 0
typedef unsigned char (*guard_fn)(void *);
struct row {
    unsigned char state;
    unsigned char event;
    guard_fn guard;
    unsigned char next;
};
static unsigned char g_count;
static unsigned char guard(void *ctx)
{
    (void)ctx;
    return g_count < 2u;
}
static const struct row table[2] = {
    { 0, 1, guard, 1 },
    { 1, 1, 0, 0 },
};
volatile unsigned char g_idx;
volatile unsigned char out;

int main(void)
{
    const struct row *r = &table[g_idx & 1];
    if (r->guard) {
        out = r->guard(0);
    }
    return 0;
}
