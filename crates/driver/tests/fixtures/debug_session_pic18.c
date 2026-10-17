int g_cnt;
int g_total;

struct P {
  int x;
  int y;
};
struct P g_p;

int main(void) {
  volatile int loc;
  g_cnt = 41;
  loc = g_cnt + 1;
  g_p.x = loc;
  g_p.y = 7;
  g_total = g_p.x + g_p.y;
  g_cnt = g_total;
  return g_cnt;
}
