//! The freestanding `<math.h>` implementation, compiled as an extra
//! translation unit when a source includes the header.
//!
//! Plain C over the f32 operators only, so clang emits the calls and
//! arithmetic the soft-float runtime already lowers; negation is spelled
//! `0.0f - x` because clang folds `-x` to the unsupported `fneg`.

pub const MATH_C: &str = r#"#include <math.h>

__attribute__((noinline)) float fabsf(float x) {
    if (x < 0.0f) return 0.0f - x;
    return x;
}

__attribute__((noinline)) float fmaxf(float a, float b) {
    if (a > b) return a;
    return b;
}

__attribute__((noinline)) float floorf(float x) {
    if (x >= 32767.0f || x <= -32768.0f) return x;
    float t = (float)(int)x;
    if (t > x) t = t - 1.0f;
    return t;
}

__attribute__((noinline)) float sqrtf(float x) {
    if (x <= 0.0f) return 0.0f;
    int k = 0;
    while (x > 4.0f && k < 30) { x = x * 0.25f; k++; }
    while (x < 1.0f && k > -30) { x = x * 4.0f; k--; }
    float y = 1.0f + (x - 1.0f) * 0.5f;
    for (int i = 0; i < 6; i++) y = (y + x / y) * 0.5f;
    while (k > 0) { y = y * 2.0f; k--; }
    while (k < 0) { y = y * 0.5f; k++; }
    return y;
}

"#;
