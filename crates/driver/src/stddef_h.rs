pub const STDDEF_H: &str = r#"#ifndef _STDDEF_H
#define _STDDEF_H

typedef int ptrdiff_t;
typedef unsigned int size_t;
typedef unsigned int wchar_t;

#ifndef NULL
#define NULL ((void*)0)
#endif

#define offsetof(type, member) ((size_t)&((type*)0)->member)

#endif /* _STDDEF_H */
"#;
