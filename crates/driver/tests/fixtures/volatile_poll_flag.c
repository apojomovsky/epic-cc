// Polling loop on a volatile flag (epic-cc#812 probe C).
// A hoisted or elided reload of `ready` hangs the loop: the load must
// stay inside it. The e2e pokes `ready` mid-run; `done` proves exit.
volatile unsigned char ready;
volatile unsigned char done;

void main(void) {
    while (!ready) {
    }
    done = 1;
}
