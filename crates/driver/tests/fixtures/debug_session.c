int g_cnt;
char g_ch;
long g_total;

struct P {
  int x;
  char c;
};
struct P g_p;

int bump(int a) {
  return a + 1;
}

int main(void) {
  volatile int arr[4];
  g_cnt = 41;
  arr[0] = bump(g_cnt);
  arr[1] = bump(arr[0]);
  g_ch = (char)arr[1];
  g_p.x = arr[0];
  g_p.c = 3;
  g_total = arr[0] + arr[1];
  g_cnt = (int)g_total;
  return arr[1];
}
