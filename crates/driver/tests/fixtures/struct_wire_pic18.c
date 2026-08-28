/* PIC18 wire-format struct layout (epic-cc#166), shaped like m-stack's
 * Chapter 9 descriptors. XC8 gives every struct member byte alignment, so
 * the mixed 8/16-bit descriptors keep their USB wire sizes; the negative-
 * array checks refuse to compile when the layout pads to natural alignment.
 * main stores the spec values and copies the raw byte image out so the test
 * can compare it against the exact wire bytes in simulator RAM. */
struct configuration_descriptor {
  unsigned char bLength;
  unsigned char bDescriptorType;
  unsigned short wTotalLength;
  unsigned char bNumInterfaces;
  unsigned char bConfigurationValue;
  unsigned char iConfiguration;
  unsigned char bmAttributes;
  unsigned char bMaxPower;
};
struct endpoint_descriptor {
  unsigned char bLength;
  unsigned char bDescriptorType;
  unsigned char bEndpointAddress;
  unsigned char bmAttributes;
  unsigned short wMaxPacketSize;
  unsigned char bInterval;
};

typedef char
    cfg_size_check[(sizeof(struct configuration_descriptor) == 9) ? 1 : -1];
typedef char
    ep_size_check[(sizeof(struct endpoint_descriptor) == 7) ? 1 : -1];

struct configuration_descriptor cfg;
struct endpoint_descriptor ep[2];
unsigned char sum;
unsigned char out_b2, out_b3, out_b8, out_sum;
unsigned char out_img7, out_img11;
unsigned short out_w, out_epw;

void main(void) {
  cfg.bLength = 9;
  cfg.bDescriptorType = 2;
  cfg.wTotalLength = 0x0022;
  cfg.bNumInterfaces = 1;
  cfg.bConfigurationValue = 1;
  cfg.iConfiguration = 0;
  cfg.bmAttributes = 0x80;
  cfg.bMaxPower = 0x32;
  ep[0].bLength = 7;
  ep[0].bDescriptorType = 5;
  ep[0].bEndpointAddress = 0x81;
  ep[0].bmAttributes = 3;
  ep[0].wMaxPacketSize = 0x0040;
  ep[0].bInterval = 1;
  ep[1].bLength = 7;
  ep[1].wMaxPacketSize = 0x0010;

  out_w = cfg.wTotalLength;
  out_epw = ep[1].wMaxPacketSize;

  /* The raw byte image is the wire format: walk it as bytes. */
  unsigned char *p = (unsigned char *)&cfg;
  out_b2 = p[2];
  out_b3 = p[3];
  out_b8 = p[8];
  sum = 0;
  for (unsigned char i = 0; i < 9; i = i + 1) {
    sum = sum + p[i];
  }
  out_sum = sum;

  /* ep[1] must start 7 bytes after ep[0], not at the align-2 stride 8. */
  unsigned char *q = (unsigned char *)ep;
  out_img7 = q[7];
  out_img11 = q[11];
}
