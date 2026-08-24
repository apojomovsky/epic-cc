// epic-cc#114 acceptance: a const struct table in flash read through a
// ccp_sel-style pointer select with a runtime instance index.
//
// The table stays `const` in source (flash), and every field read goes
// through a pointer returned by `ccp_sel` (the select shape clang -O1
// emits for the HAL's static helper) and by `ccp_sel_oi` (the same body
// kept out-of-line, the call-return shape legalize sinks). The reads are
// volatile so clang cannot fold the table contents into constants, and the
// index is a runtime global so the RETLW readers and the fold actually run.
struct ccp_addrs {
    unsigned char cprl;
    unsigned char cprh;
    unsigned char con;
    unsigned char irq;
};

static const struct ccp_addrs addrs[2] = {
    { 0x15U, 0x16U, 0x17U, 0x01U },
    { 0x1BU, 0x1CU, 0x1DU, 0x02U },
};

volatile unsigned char inst;
volatile unsigned char out_cprl, out_cprh, out_con, out_irq;
volatile unsigned char out_cprl2, out_cprh2, out_con2, out_irq2;

static const struct ccp_addrs *ccp_sel(unsigned char i)
{
    if (i == 1) return &addrs[1];
    return &addrs[0];
}

__attribute__((noinline)) static const struct ccp_addrs *ccp_sel_oi(unsigned char i)
{
    if (i == 1) return &addrs[1];
    return &addrs[0];
}

void main(void)
{
    const struct ccp_addrs *a = ccp_sel(inst);
    out_cprl = ((volatile const unsigned char *)a)[0];
    out_cprh = ((volatile const unsigned char *)a)[1];
    out_con = ((volatile const unsigned char *)a)[2];
    out_irq = ((volatile const unsigned char *)a)[3];

    const struct ccp_addrs *b = ccp_sel_oi(inst);
    out_cprl2 = ((volatile const unsigned char *)b)[0];
    out_cprh2 = ((volatile const unsigned char *)b)[1];
    out_con2 = ((volatile const unsigned char *)b)[2];
    out_irq2 = ((volatile const unsigned char *)b)[3];
}
