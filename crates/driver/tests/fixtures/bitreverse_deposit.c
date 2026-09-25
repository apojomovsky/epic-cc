// Gate B for epic-cc#679: clang's 2-bit deposit idiom (the shape that
// surfaces as `llvm.bitreverse.i6` in the encoder sim harness) deposits
// correctly on hardware. Each `deposit` writes one 2-bit state to pins
// 4/5 exactly the way `port_byte` does; the test writes all four states
// through volatile so the idiom survives optimization and reads back
// the port bytes. Expected: {0x00, 0x20, 0x10, 0x30}, halted.

static volatile unsigned char g_ports[4];
volatile unsigned char g_pick;

static unsigned char deposit(unsigned char state)
{
    unsigned char v = 0u;
    if (state & 0x2u)
    {
        v = (unsigned char)(v | (unsigned char)(1u << 4u));
    }
    if (state & 0x1u)
    {
        v = (unsigned char)(v | (unsigned char)(1u << 5u));
    }
    return v;
}

void main(void)
{
    unsigned char pick = g_pick;
    for (unsigned char s = 0u; s < 4u; s++)
    {
        g_ports[(unsigned char)(s + pick)] = deposit((unsigned char)(s + pick));
    }
}
