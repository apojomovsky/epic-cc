int f() {
    try {
        return 1;
    } catch (...) {
        return 0;
    }
}

int main() { return f(); }
