/* Chained bitmask lanes (epic-cc#763): the bench-bitmask shape with a
 * mixed taken/untaken input pattern, so every lane form (const default,
 * or-select taken and skipped, or-bool tail) executes on one shared
 * accumulator slot per chain.
 *
 * g_cfg = {Mode=1, DataWidth=0, BaudHigh=1, AutoBaud=1, AddressDetect=1,
 * TxCallback=1, RxCallback=0}: out_txsta = 0x02|0x10|0x04|0x20 = 0x36,
 * out_rcsta = 0x80|0x08 = 0x88, out_baudcon = 0x08|0x01 = 0x09. */
typedef unsigned char uint8_t;

typedef struct {
    uint8_t Mode;
    uint8_t DataWidth;
    uint8_t BaudHigh;
    uint8_t AutoBaud;
    uint8_t AddressDetect;
    uint8_t TxCallback;
    uint8_t RxCallback;
} UartConfig;

volatile UartConfig g_cfg = {1, 0, 1, 1, 1, 1, 0};
volatile unsigned char out_txsta;
volatile unsigned char out_rcsta;
volatile unsigned char out_baudcon;

void main(void) {
    uint8_t txsta = 0x02U;
    if (g_cfg.Mode == 1U) txsta |= 0x10U;
    if (g_cfg.BaudHigh == 1U) txsta |= 0x04U;
    if (g_cfg.DataWidth == 1U) txsta |= 0x40U;
    if (g_cfg.TxCallback) txsta |= 0x20U;
    uint8_t rcsta = 0x80U;
    if (g_cfg.DataWidth == 1U) rcsta |= 0x40U;
    if (g_cfg.AddressDetect) rcsta |= 0x08U;
    if (g_cfg.RxCallback) rcsta |= 0x10U;
    out_txsta = txsta;
    out_rcsta = rcsta;
    uint8_t baudcon = 0x00U;
    if (g_cfg.BaudHigh == 1U) baudcon |= 0x08U;
    if (g_cfg.AutoBaud) baudcon |= 0x01U;
    out_baudcon = baudcon;
}
