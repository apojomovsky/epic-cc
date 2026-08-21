asm("my_label: nop");
void main(void) { asm volatile("goto my_label"); }
