//! The freestanding `<math.h>` declarations epic-cc ships to user code.
//! Only the f32 routines the soft-float runtime can back; `double` is
//! `float` on this target, so no separate double symbols are needed.

pub const MATH_H: &str = r#"#ifndef _MATH_H
#define _MATH_H

float sqrtf(float x);
float fabsf(float x);
float fmaxf(float a, float b);
float floorf(float x);

#endif /* _MATH_H */
"#;
