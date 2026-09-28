// Every __interrupt spelling (epic-cc#688): empty, high/low priority and
// numeric all reach clang's interrupt attribute (checked in xc_headers_e2e).
__interrupt() void isr_empty(void) { }
__interrupt(high_priority) void isr_hi(void) { }
__interrupt(low_priority) void isr_lo(void) { }
__interrupt(2) void isr_num(void) { }
void main(void) { }
