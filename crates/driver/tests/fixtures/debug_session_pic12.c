volatile int g_in;
volatile int g_cnt;

int main(void) {
  int loc;
  g_cnt = (int)g_in;
  loc = (int)g_cnt + 1;
  g_cnt = loc;
  return loc;
}
