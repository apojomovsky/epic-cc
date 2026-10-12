/* u16 right-justified decimal tails: the menu-redraw shape shared as
 * `__udec_u16_rj5` calls (epic-cc#927, #1055). Backward digits plus
 * blank fill form the header/blank/latch triple after LSR; two
 * width-5 sites share, the width-4 site stays expanded (the helper
 * is hardcoded to 5 digits). Mixed widths are load-bearing: one
 * width everywhere canonicalizes to the dual form the matcher does
 * not cover (pinned clang keeps the choice deterministic). */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

#define LCD_COLS 16u

static char g_line_buf[LCD_COLS + 1u];

volatile uint16_t vin_a;
volatile uint16_t vin_b;
volatile uint16_t vin_c;
volatile uint8_t sink;

#define BUILD_LINE(prefix_literal) do {                               \
    static const char epic_menu_lit_[] = (prefix_literal);            \
    uint8_t epic_menu_i_;                                              \
    for (epic_menu_i_ = 0;                                            \
         epic_menu_i_ < LCD_COLS &&                                    \
         epic_menu_i_ < (uint8_t)(sizeof(epic_menu_lit_) - 1u);        \
         epic_menu_i_++)                                               \
    {                                                                  \
        g_line_buf[epic_menu_i_] = epic_menu_lit_[epic_menu_i_];       \
    }                                                                  \
    for (; epic_menu_i_ < LCD_COLS; epic_menu_i_++)                    \
    {                                                                  \
        g_line_buf[epic_menu_i_] = ' ';                                \
    }                                                                  \
    g_line_buf[LCD_COLS] = '\0';                                       \
} while (0)

static void put_u16_tail(uint16_t v, uint8_t width)
{
    uint8_t i;
    uint8_t end = LCD_COLS;
    for (i = 0; i < width; i++)
    {
        g_line_buf[end - 1U - i] = (char)('0' + (v % 10U));
        v /= 10U;
        if (v == 0U && i + 1U < width)
        {
            uint8_t j;
            for (j = (uint8_t)(i + 1U); j < width; j++)
            {
                g_line_buf[end - 1U - j] = ' ';
            }
            break;
        }
    }
}

static void redraw_status(void)
{
    BUILD_LINE("Ticks:");
    put_u16_tail(vin_a, 5U);
    put_u16_tail(vin_b, 4U);
    sink = g_line_buf[0];
}

static void redraw_about(void)
{
    BUILD_LINE("EEwr:");
    put_u16_tail(vin_c, 5U);
    sink = g_line_buf[1];
}

void main(void)
{
    redraw_status();
    redraw_about();
}
