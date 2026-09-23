/* Bitmask build from config struct fields through if-chains.
 * Menu-demo triage (epic-cc#624): EPIC_USART_Init (445 vs XC8 181)
 * builds TXSTA/RCSTA/BAUDCON bitmasks from one if-chain per bit,
 * reloading the same handle fields for every test. */
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

volatile UartConfig g_cfg;
volatile unsigned char g_txsta;
volatile unsigned char g_rcsta;
volatile unsigned char g_baudcon;

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
    g_txsta = txsta;
    g_rcsta = rcsta;
    uint8_t baudcon = 0x00U;
    if (g_cfg.BaudHigh == 1U) baudcon |= 0x08U;
    if (g_cfg.AutoBaud) baudcon |= 0x01U;
    g_baudcon = baudcon;
}
