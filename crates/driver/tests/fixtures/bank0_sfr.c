// Issue #112 regression: a non-mirrored bank-0 SFR access after a banked
// operand must select bank 0 first. `REG8(0x85)` is TRISA (bank 1),
// `REG8(0x05)` is PORTA (bank 0). Without the fix the second store emits
// no BANKSEL and lands on TRISA again (the RP bits are still bank 1).
#define REG8(a) (*(volatile unsigned char *)(a))
int main(void) {
    REG8(0x85) = 0x00;   /* TRISA, bank 1 */
    REG8(0x05) = 0x01;   /* PORTA, bank 0 */
    return 0;
}
