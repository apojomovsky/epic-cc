use ir::parse;
use iselcore::{add_int_flash_definite, flash_provenance, resolve_pointers};

fn main() {
    let ir = "const c i8\nglobal g i16\nglobal h i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    store ptr @c @g\n    %x = load i16 @g\n    store i16 %x @h\n    %y = load i16 @h\n    %p = inttoptr i16 %y to ptr\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n";
    let mut m = parse(ir);
    for g in &mut m.globals {
        if g.name == "c" {
            g.bytes = vec![b'P'];
            g.size = 1;
        }
    }
    let resolved = resolve_pointers(&m);
    let mut prov = flash_provenance(&m);
    let isf = |n: &str| m.globals.iter().any(|g| g.name == n && g.is_const);
    add_int_flash_definite(&mut prov, &m, &resolved, isf);
    println!("int_flash_ptr={:?}", prov.int_flash_ptr);
}
