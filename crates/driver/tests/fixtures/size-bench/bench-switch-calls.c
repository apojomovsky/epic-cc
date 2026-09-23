/* Switch dispatching calls with distinct constant arguments.
 * Menu-demo triage (epic-cc#617): the redraw cluster (switch on screen
 * id, one helper call per case) is 669 vs XC8 ~235 words. The existing
 * bench-switch covers value stores per case; this one covers calls. */
volatile unsigned char in;
volatile unsigned char out;

static void show_status(unsigned char arg) { out = arg; }
static void show_brightness(unsigned char arg) { out = (unsigned char)(arg + 1); }
static void show_about(unsigned char arg) { out = (unsigned char)(arg + 2); }

void main(void) {
    switch (in) {
    case 0: show_status(10); break;
    case 1: show_brightness(20); break;
    case 2: show_about(30); break;
    case 3: show_status(40); break;
    default: out = 0; break;
    }
}
