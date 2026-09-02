// epic-cc#155 acceptance: an indirect call through a function pointer stored
// in a struct, with a runtime pointer arg (the epic-taskmgr `t->fn(t->arg)`
// shape). The arg is a `load ptr` result (the TCB's arg field), not a
// compile-time address: isel must copy the loaded 2 bytes into the callee's
// param slot and let the callee's FSR-based deref resolve the address at
// runtime. Before the fix both backends panicked ("no gep for pointer").
//
//   g_sel == 1 -> run_once(&g_tasks[0]) -> task_blink(g_tasks[0].arg)
//              -> g_seen = *g_payload = 0xAB
//   g_sel == 0 -> nothing; g_seen stays 0
typedef void (*task_fn)(void *);
struct tcb {
    task_fn fn;
    void *arg;
};
static struct tcb g_tasks[1];
static unsigned char g_payload;
volatile static unsigned char g_seen;
volatile unsigned char g_sel;
__attribute__((noinline)) static void task_blink(void *arg) {
    g_seen = *(unsigned char *)arg;
}
__attribute__((noinline)) static void run_once(struct tcb *t) {
    t->fn(t->arg);
}
int main(void) {
    g_tasks[0].fn = task_blink;
    g_tasks[0].arg = &g_payload;
    g_payload = 0xAB;
    if (g_sel) run_once(&g_tasks[0]);
    return 0;
}
