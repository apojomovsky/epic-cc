int g_int;
char g_char;
long g_long;

struct P {
  int x;
  char c;
};
struct P g_p;

int main(void) {
  int slot = 0;
  int *p = &slot;
  int arr[4];
  arr[0] = g_int;
  static volatile int st = 5;
  g_int = st;
  g_char = (char)*p;
  g_long = *p;
  g_p.x = *p;
  g_p.c = 1;
  return arr[0] + (int)g_long;
}
