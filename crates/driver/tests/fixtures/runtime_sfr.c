// epic-cc#117 acceptance: volatile SFR access through a RUNTIME address,
// mirroring the real pic16_irq.c shapes (epic-hal#67 item 2).
//
// The three shapes the pinned clang -O1 emits for `pir_reg_addr(d)`:
//   1. standalone runtime `inttoptr` (read_offset: addr = 0x0C + (off & 1));
//   2. pointer select over two literal inttoptrs (GetFlag/ClearFlag's
//      `pir_is_pir2 ? PIR2 : PIR1`);
//   3. pointer phi joining the select result and the INTCON literal
//      (GetFlag's `in_intcon ? INTCON : addr` join).
// Every read/write is volatile so clang cannot fold the addresses, and the
// `irq` global is preloaded by the simulator so both PIR1 (0x0C) and PIR2
// (0x0D) arms actually run.
//
// Hand-computed expectations (sim sets `irq` before run):
//   PIR1 = 0x0C, PIR2 = 0x0D, INTCON = 0x0B (the F877A SFRs).
//   irq = 0 (RB, in_intcon=1): GetFlag reads INTCON (shape 3).
//   irq = 2 (TMR1, PIR1): GetFlag reads PIR1 (shape 2); ClearFlag too.
//   irq = 4 (BCL, PIR2): GetFlag reads PIR2 (shape 2); ClearFlag too.
// The sim preloads PIR1/PIR2/INTCON bytes and the program clears/sets
// bits, so the observable results prove the right SFR was touched.
// Freestanding: no stdint.h (the driver's header dir is only wired into
// the CLI path; the layout helper compiles with Options::default).
typedef unsigned char uint8_t;

#define EPIC_REG8(addr) (*(volatile uint8_t *)(addr))

#define PIC_REG_INTCON 0x0B
#define PIC_REG_PIR1 0x0C
#define PIC_REG_PIR2 0x0D

#define PIC_INTCON_RBIF 0x08
#define PIC_INTCON_INTF 0x01
#define PIC_PIR1_TMR1IF 0x01
#define PIC_PIR1_TMR2IF 0x02
#define PIC_PIR2_BCLIF 0x01
#define PIC_PIR2_CCP2IF 0x02

typedef struct {
    uint8_t flag_mask;
    uint8_t in_intcon;
    uint8_t pir_is_pir2;
} irq_desc_t;

static const irq_desc_t irq_table[6] = {
    { PIC_INTCON_RBIF, 1, 0 }, /* RB   : INTCON */
    { PIC_INTCON_INTF, 1, 0 }, /* INT  : INTCON */
    { PIC_PIR1_TMR1IF, 0, 0 }, /* TMR1 : PIR1   */
    { PIC_PIR1_TMR2IF, 0, 0 }, /* TMR2 : PIR1   */
    { PIC_PIR2_BCLIF,  0, 1 }, /* BCL  : PIR2   */
    { PIC_PIR2_CCP2IF, 0, 1 }, /* CCP2 : PIR2   */
};

#define IRQ_TABLE_SIZE (sizeof irq_table / sizeof irq_table[0])

#define pir_reg_addr(d) ((d)->pir_is_pir2 ? PIC_REG_PIR2 : PIC_REG_PIR1)

volatile uint8_t irq;
volatile uint8_t out_flag;
volatile uint8_t out_clear;
volatile uint8_t out_write;

/* Shape 2 + 3: the real HAL function shapes, shared implementation. */
uint8_t EPIC_IRQ_GetFlag(void)
{
    const irq_desc_t *d = &irq_table[irq & 7];
    uint8_t in_intcon = d->in_intcon;
    uint8_t flag_mask = d->flag_mask;
    uint8_t addr = pir_reg_addr(d);
    uint8_t reg = in_intcon ? EPIC_REG8(PIC_REG_INTCON) : EPIC_REG8(addr);
    return (reg & flag_mask) ? 1U : 0U;
}

void EPIC_IRQ_ClearFlag(void)
{
    const irq_desc_t *d = &irq_table[irq & 7];
    uint8_t in_intcon = d->in_intcon;
    uint8_t flag_mask = d->flag_mask;
    if (in_intcon) {
        uint8_t v = EPIC_REG8(PIC_REG_INTCON);
        v &= (uint8_t)~flag_mask;
        EPIC_REG8(PIC_REG_INTCON) = v;
    } else {
        uint8_t addr = pir_reg_addr(d);
        uint8_t v = EPIC_REG8(addr);
        v &= (uint8_t)~flag_mask;
        EPIC_REG8(addr) = v;
    }
}

/* Shape 1: a standalone runtime inttoptr (table-free computed address). */
uint8_t read_offset(uint8_t off)
{
    uint8_t addr = (uint8_t)(PIC_REG_PIR1 + (off & 0x01));
    return EPIC_REG8(addr);
}

void write_offset(uint8_t off, uint8_t v)
{
    uint8_t addr = (uint8_t)(PIC_REG_PIR1 + (off & 0x01));
    EPIC_REG8(addr) = v;
}

void main(void)
{
    out_flag = EPIC_IRQ_GetFlag();
    EPIC_IRQ_ClearFlag();
    out_clear = EPIC_REG8(PIC_REG_PIR1) | EPIC_REG8(PIC_REG_PIR2);
    out_flag = (uint8_t)(out_flag + EPIC_IRQ_GetFlag());
    out_flag = (uint8_t)(out_flag + read_offset(irq & 1));
    out_flag = (uint8_t)(out_flag + read_offset(irq & 1));
    write_offset(irq & 1, 0xAA);
    out_write = EPIC_REG8(PIC_REG_PIR1);
}
