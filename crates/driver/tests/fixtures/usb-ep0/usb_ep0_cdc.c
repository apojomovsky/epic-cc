/* A minimal USB CDC-ACM device stack for endpoint 0, the device half of the
 * enumeration walk in crates/driver/tests/usb_ep0_e2e.rs. It speaks
 * M-Stack's endpoint 0 protocol, the one epic-hal's lib/usb wraps, and
 * serves the descriptor bytes that example ships.
 */

#include <stdint.h>
#include <epic-cc.h>

#define EPIC_REG8(addr) (*(volatile uint8_t *)(uintptr_t)(addr))

/* PIC18F4550 USB SFRs and bits (DS39632E chapter 17). */
#define USB_UCON   0xF6DU
#define USB_USTAT  0xF6CU
#define USB_UIE    0xF69U
#define USB_UIR    0xF68U
#define USB_UEIE   0xF6BU
#define USB_UADDR  0xF6EU
#define USB_UCFG   0xF6FU
#define USB_UEP0   0xF70U

#define UCON_USBEN   0x08U
#define UCON_PKTDIS  0x10U
#define UIR_URSTIF   0x01U
#define UIR_TRNIF    0x08U
#define UIR_STALLIF  0x20U
#define UEP_EPSTALL  0x01U
#define UEP_EPINEN   0x02U
#define UEP_EPOUTEN  0x04U
#define UEP_EPHSHK   0x10U

#define UCFG_FSEN    0x04U
#define UCFG_UPUEN   0x10U

#define USTAT_ENDP_MASK 0x78U
#define USTAT_DIR_IN    0x04U

#define BD_UOWN      0x80U
#define BD_DTS       0x40U
#define BD_DTSEN     0x08U
#define BD_BSTALL    0x04U

/* USB PIDs, usb_ch9.h; the SIE writes them into BDnSTAT<5:2>. */
#define PID_SETUP 0x0DU

/* Standard requests, usb_ch9.h. */
#define GET_STATUS        0x00U
#define SET_ADDRESS       0x05U
#define GET_DESCRIPTOR    0x06U
#define GET_CONFIGURATION 0x08U
#define SET_CONFIGURATION 0x09U
#define GET_INTERFACE     0x0AU
#define SET_INTERFACE     0x0BU

#define DESC_DEVICE     0x01U
#define DESC_CONFIG     0x02U
#define DESC_STRING     0x03U

/* CDC class requests, usb_cdc.h. */
#define CDC_SET_LINE_CODING        0x20U
#define CDC_GET_LINE_CODING        0x21U
#define CDC_SET_CONTROL_LINE_STATE 0x22U

#define REQ_TYPE_CLASS 0x20U

#define EP0_LEN 8U

/* Endpoint 0's descriptors: no ping-pong (UCFG.PPB = 00), so EP0 OUT is BD0
 * at 0x400 and EP0 IN is BD1 at 0x404, DS39632E table 17-4. */
static volatile uint8_t bd0_out[4] EPIC_AT(0x400);
static volatile uint8_t bd0_in[4] EPIC_AT(0x404);

/* The endpoint 0 buffers, in USB RAM (DS39632E 17.3). */
static volatile uint8_t ep0_out[EP0_LEN] EPIC_AT(0x500);
static volatile uint8_t ep0_in[EP0_LEN] EPIC_AT(0x508);

/* The descriptors epic-hal's lib/usb ships. */
static uint8_t device_desc[18] EPIC_AT(0x520);
static uint8_t config_desc[70] EPIC_AT(0x540);
static uint8_t string0[4] EPIC_AT(0x590);
static uint8_t string1[34] EPIC_AT(0x598);
static uint8_t string2[18] EPIC_AT(0x5C0);
static uint8_t string3[60] EPIC_AT(0x5D8);

/* Observable state, read off the compiler's --map by the test. */
volatile uint8_t g_configured;
volatile uint8_t g_dtr;
volatile uint8_t g_line_coding[7];
volatile uint8_t g_ep0_stalls;

/* The stage of the control transfer in flight. */
#define STAGE_IDLE      0U
#define STAGE_IN_DATA   1U /* device -> host */
#define STAGE_IN_STATUS 2U /* the zero-length packet ending an OUT transfer */
#define STAGE_OUT_DATA  3U /* host -> device, collected in out_buf */

static uint8_t stage;
static const uint8_t *in_src; /* next descriptor byte to send */
static uint8_t in_left;       /* descriptor bytes still unsent */
static uint8_t in_data1;      /* toggle of the next IN data packet */
static uint8_t returning_short;
static uint8_t need_zlp;
static uint8_t out_len;
static uint8_t out_buf[EP0_LEN];
static uint8_t pending_addr;
static uint8_t addr_pending;

/* The copy loop lives in a `static` helper, not inline in its callers: an
 * inlined loop's induction variable takes an overlay slot, and on the
 * PIC18 backend that slot can be the one a caller's own local still holds
 * (epic-cc#1417), which makes the loop count from a stale value. A real
 * call keeps the induction variable in this frame. */
static void copy_bytes(volatile uint8_t *dst, const uint8_t *src, uint8_t len)
{
    uint8_t i;
    for (i = 0; i < len; i++) {
        dst[i] = src[i];
    }
}

/* Hand one IN packet to the SIE: `len` bytes from `src`, or from the
 * caller-filled ep0_in buffer when `src` is null. `data1` is the packet's
 * toggle; a control transfer's first data packet and its status stage are
 * DATA1, the packets between them alternate. */
static void arm_in(const uint8_t *src, uint8_t len, uint8_t data1)
{
    uint8_t i;
    if (src != 0) {
        for (i = 0; i < len; i++) {
            ep0_in[i] = src[i];
        }
    }
    bd0_in[1] = len;
    bd0_in[2] = 0x08; /* 0x508 */
    bd0_in[3] = 0x05;
    bd0_in[0] = BD_UOWN | BD_DTSEN | (data1 ? BD_DTS : 0);
}

static void stall_ep0(void)
{
    bd0_in[1] = EP0_LEN;
    bd0_in[0] = BD_UOWN | BD_BSTALL;
    g_ep0_stalls++;
}

/* Start an IN data stage from a descriptor, M-Stack's start_control_return:
 * one packet now, the rest after each IN completion, a zero-length packet
 * last when the host asked for more than the descriptor holds. */
static void start_in(const uint8_t *src, uint8_t len, uint8_t asked)
{
    uint8_t n = len;
    if (n > EP0_LEN) {
        n = EP0_LEN;
    }
    if (n > asked) {
        n = asked;
    }
    returning_short = (len < asked) ? 1 : 0;
    in_src = src + n;
    in_left = (uint8_t)(((asked < len) ? asked : len) - n);
    need_zlp = (uint8_t)(in_left == 0 && returning_short);
    stage = STAGE_IN_DATA;
    /* A control transfer's data stage starts with DATA1 and alternates. */
    in_data1 = 1;
    arm_in(src, n, in_data1);
    in_data1 = (uint8_t)!in_data1;
}

static void next_in_packet(void)
{
    uint8_t n;

    if (in_left != 0) {
        n = (in_left > EP0_LEN) ? EP0_LEN : in_left;
        in_left = (uint8_t)(in_left - n);
        if (in_left == 0 && n == EP0_LEN && returning_short) {
            need_zlp = 1;
        }
        arm_in(in_src, n, in_data1);
        in_data1 = (uint8_t)!in_data1;
        in_src += n;
        return;
    }
    if (need_zlp) {
        need_zlp = 0;
        arm_in(0, 0, in_data1);
        in_data1 = (uint8_t)!in_data1;
        return;
    }
    stage = STAGE_IDLE;
}

static void send_zlp_status(void)
{
    stage = STAGE_IN_STATUS;
    arm_in(0, 0, 1);
}

/* A one- or two-byte answer staged through ep0_in. */
static void answer_short(uint8_t lo, uint8_t hi, uint8_t len)
{
    ep0_in[0] = lo;
    ep0_in[1] = hi;
    in_left = 0;
    need_zlp = 0;
    returning_short = 1;
    stage = STAGE_IN_DATA;
    arm_in(0, len, 1);
}

static void handle_standard_in(uint16_t value, uint16_t length);
static void handle_standard_out(uint16_t value);

static void handle_setup(void)
{
    /* PIC18 silicon leaves BSTALL and UOWN set across a new SETUP, so the
     * stack clears a previous stall here (M-Stack's NEEDS_CLEAR_STALL). */
    bd0_in[1] = EP0_LEN;
    bd0_in[0] = 0;

    /* The SIE wrote the packet into the EP0 OUT buffer, DS39632E 17.4.1.
     *
     * The request byte is read at each point of use rather than into a
     * local: a local that stays live across the whole function takes an
     * overlay slot the inlined copy loop wants for its induction variable,
     * and the loop then starts from the stale byte (epic-cc#1417). */
    uint16_t value = (uint16_t)ep0_out[2] | ((uint16_t)ep0_out[3] << 8);
    uint16_t length = (uint16_t)ep0_out[6] | ((uint16_t)ep0_out[7] << 8);

    if ((ep0_out[0] & 0x60U) == REQ_TYPE_CLASS) {
        if (ep0_out[1] == CDC_SET_LINE_CODING) {
            stage = STAGE_OUT_DATA;
            out_len = 0;
        } else if (ep0_out[1] == CDC_GET_LINE_CODING) {
            copy_bytes(ep0_in, (const uint8_t *)g_line_coding, 7);
            in_left = 0;
            need_zlp = 0;
            returning_short = 1;
            stage = STAGE_IN_DATA;
            arm_in(0, 7, 1);
        } else if (ep0_out[1] == CDC_SET_CONTROL_LINE_STATE) {
            g_dtr = (uint8_t)(value & 0x01U);
            send_zlp_status();
        } else {
            stall_ep0();
        }
        return;
    }

    /* The device-to-host form of a standard request has the direction bit
     * set, the host-to-device form clear; a request with the wrong
     * direction is one no device can answer, so each direction gets its
     * own handler. */
    if (ep0_out[0] == 0x80U) {
        handle_standard_in(value, length);
    } else {
        handle_standard_out(value);
    }
}

/* A standard request the host reads from the device. */
static void handle_standard_in(uint16_t value, uint16_t length)
{
    switch (ep0_out[1]) {
    case GET_DESCRIPTOR: {
        uint8_t type = (uint8_t)(value >> 8);
        uint8_t idx = (uint8_t)value;
        if (type == DESC_DEVICE) {
            start_in(device_desc, sizeof(device_desc), (uint8_t)length);
        } else if (type == DESC_CONFIG) {
            start_in(config_desc, sizeof(config_desc), (uint8_t)length);
        } else if (type == DESC_STRING) {
            /* One branch per string keeps the descriptor address a constant
             * operand; a table of pointers would need a flash-resident
             * pointer array the PIC18 backend does not model. */
            if (idx == 0) {
                start_in(string0, sizeof(string0), (uint8_t)length);
            } else if (idx == 1) {
                start_in(string1, sizeof(string1), (uint8_t)length);
            } else if (idx == 2) {
                start_in(string2, sizeof(string2), (uint8_t)length);
            } else if (idx == 3) {
                start_in(string3, sizeof(string3), (uint8_t)length);
            } else {
                stall_ep0();
            }
        } else {
            stall_ep0();
        }
        break;
    }
    case GET_CONFIGURATION:
        answer_short(g_configured, 0, 1);
        break;
    case GET_STATUS:
        answer_short(0, 0, 2);
        break;
    case GET_INTERFACE:
        answer_short(0, 0, 1);
        break;
    default:
        stall_ep0();
        break;
    }
}

/* A standard request the host writes to the device. */
static void handle_standard_out(uint16_t value)
{
    switch (ep0_out[1]) {
    case SET_ADDRESS:
        /* The address takes effect after the status stage, DS39632E 17.2.5. */
        pending_addr = (uint8_t)value;
        addr_pending = 1;
        send_zlp_status();
        break;
    case SET_CONFIGURATION:
        g_configured = (uint8_t)value;
        send_zlp_status();
        break;
    case SET_INTERFACE:
        send_zlp_status();
        break;
    default:
        stall_ep0();
        break;
    }
}

static void handle_ep0_out(uint8_t len)
{
    uint8_t i;

    if (stage == STAGE_IN_DATA) {
        /* An empty OUT packet on an IN transfer is its status stage. */
        stage = STAGE_IDLE;
        return;
    }
    if (stage == STAGE_OUT_DATA) {
        for (i = 0; i < len && out_len < EP0_LEN; i++) {
            out_buf[out_len++] = ep0_out[i];
        }
        for (i = 0; i < 7 && i < out_len; i++) {
            g_line_coding[i] = out_buf[i];
        }
        stage = STAGE_IDLE;
        send_zlp_status();
    }
}

static void handle_ep0_in(void)
{
    if (addr_pending) {
        EPIC_REG8(USB_UADDR) = pending_addr;
        addr_pending = 0;
    }
    if (stage == STAGE_IN_DATA) {
        next_in_packet();
    } else {
        stage = STAGE_IDLE;
    }
}

static void rearm_out(void)
{
    bd0_out[1] = EP0_LEN;
    bd0_out[2] = 0x00; /* 0x500 */
    bd0_out[3] = 0x05;
    bd0_out[0] = BD_UOWN;
}

static void usb_init(void)
{
    uint8_t i;

    EPIC_REG8(USB_UCON) = 0;
    EPIC_REG8(USB_UCFG) = UCFG_FSEN | UCFG_UPUEN;
    EPIC_REG8(USB_UIE) = 0;
    EPIC_REG8(USB_UEIE) = 0;
    EPIC_REG8(USB_UIR) = 0;
    EPIC_REG8(USB_UCON) = UCON_USBEN;

    for (i = 0; i < 16; i++) {
        EPIC_REG8(USB_UEP0 + i) = 0;
    }
    EPIC_REG8(USB_UEP0) = UEP_EPHSHK | UEP_EPINEN | UEP_EPOUTEN;
    EPIC_REG8(USB_UADDR) = 0;

    bd0_in[1] = EP0_LEN;
    bd0_in[2] = 0x08;
    bd0_in[3] = 0x05;
    bd0_in[0] = 0;
    rearm_out();

    stage = STAGE_IDLE;
    in_left = 0;
    need_zlp = 0;
    addr_pending = 0;
    g_configured = 0;
    g_dtr = 0;
}

static void usb_service(void)
{
    uint8_t uir = EPIC_REG8(USB_UIR);

    if (uir & UIR_URSTIF) {
        EPIC_REG8(USB_UIR) &= (uint8_t)~UIR_URSTIF;
        usb_init();
        return;
    }
    if (uir & UIR_STALLIF) {
        EPIC_REG8(USB_UIR) &= (uint8_t)~UIR_STALLIF;
        EPIC_REG8(USB_UEP0) &= (uint8_t)~UEP_EPSTALL;
    }
    if (uir & UIR_TRNIF) {
        uint8_t ustat = EPIC_REG8(USB_USTAT);
        if ((ustat & USTAT_ENDP_MASK) == 0) {
            if (ustat & USTAT_DIR_IN) {
                handle_ep0_in();
            } else if ((bd0_out[0] >> 2) == PID_SETUP) {
                handle_setup();
            } else {
                handle_ep0_out(bd0_out[1]);
            }
            rearm_out();
        }
        EPIC_REG8(USB_UIR) &= (uint8_t)~UIR_TRNIF;
        EPIC_REG8(USB_UCON) &= (uint8_t)~UCON_PKTDIS;
    }
}

static void load_descriptors(void)
{
    static const uint8_t dev[18] = {
        18, DESC_DEVICE, 0x00, 0x02,
        0xEF, 0x02, 0x01, EP0_LEN,
        0xA0, 0xA0, 0x04, 0x00, 0x01, 0x00, /* idVendor, idProduct, bcdDevice 1.00 */
        1, 2, 3, 1
    };
    static const uint8_t cfg[70] = {
        9, DESC_CONFIG, sizeof(cfg), 0,
        2, 1, 0, 0x80, 50,
        8, 0x0B, 0, 2, 0x02, 0x02, 0, 0,
        9, 0x04, 0, 0, 1, 0x02, 0x02, 0, 0,
        5, 0x24, 0x00, 0x10, 0x01,
        4, 0x24, 0x02, 0x06,
        5, 0x24, 0x06, 0, 1,
        7, 0x05, 0x81, 0x03, 10, 0, 1,
        9, 0x04, 1, 0, 2, 0x0A, 0, 0, 0,
        7, 0x05, 0x82, 0x02, 64, 0, 1,
        7, 0x05, 0x02, 0x02, 64, 0, 1
    };
    static const uint8_t s0[4] = { 4, DESC_STRING, 0x09, 0x04 };
    static const uint8_t s1[34] = {
        34, DESC_STRING,
        'p', 0, 'i', 0, 'c', 0, '8', 0, '-', 0, 'u', 0, 's', 0,
        'b', 0, ' ', 0, 'p', 0, 'r', 0, 'o', 0, 'j', 0, 'e', 0,
        'c', 0, 't', 0
    };
    static const uint8_t s2[18] = {
        18, DESC_STRING,
        'p', 0, 'i', 0, 'c', 0, '8', 0, '-', 0, 'u', 0, 's', 0, 'b', 0
    };
    static const uint8_t s3[60] = {
        60, DESC_STRING,
        'D', 0, 'E', 0, 'V', 0, ' ', 0, 'B', 0, 'U', 0, 'I', 0, 'L', 0,
        'D', 0, ' ', 0, '-', 0, ' ', 0, 'N', 0, 'O', 0, 'T', 0, ' ', 0,
        'F', 0, 'O', 0, 'R', 0, ' ', 0, 'S', 0, 'H', 0, 'I', 0, 'P', 0,
        'P', 0, 'I', 0, 'N', 0, 'G', 0, '.', 0
    };
    copy_bytes(device_desc, dev, sizeof(dev));
    copy_bytes(config_desc, cfg, sizeof(cfg));
    copy_bytes(string0, s0, sizeof(s0));
    copy_bytes(string1, s1, sizeof(s1));
    copy_bytes(string2, s2, sizeof(s2));
    copy_bytes(string3, s3, sizeof(s3));
}

int main(void)
{
    load_descriptors();
    usb_init();
    for (;;) {
        usb_service();
    }
}
