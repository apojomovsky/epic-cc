struct Topic {
  unsigned char id;
  const char *name;
};

unsigned char sink(const struct Topic *t) { return t[0].id; }
