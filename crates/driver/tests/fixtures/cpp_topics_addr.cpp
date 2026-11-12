// A namespace-scope const table in C++ has internal linkage, so clang
// names it with the Itanium mangling below (_ZL8topics_a). The table is
// both passed as a plain call argument and bound into a RAM pointer that
// outlives the call, the ros2serial static-sketch shape (epic-cc#1478).
struct Topic {
  unsigned char id;
  const char *name;
};

extern "C" unsigned char sink(const Topic *t);

const Topic topics_a[] = {{16, "battery"}, {17, "cmd_vel"}};
const Topic topics_b[] = {{3, "imu"}};

static const Topic *volatile bound;

extern "C" void bind_direct(void) { bound = topics_a; }

volatile unsigned char out_id;
volatile unsigned char out_sink;

int main() {
  out_sink = sink(topics_a);
  out_sink = (unsigned char)(out_sink + sink(topics_b));
  bind_direct();
  out_id = bound[1].id;
  return 0;
}
