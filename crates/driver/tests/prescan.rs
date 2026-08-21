use driver::prescan::find_epic_config;

fn src(s: &[(&str, &str)]) -> Vec<(String, String)> {
    s.iter()
        .map(|(n, c)| (n.to_string(), c.to_string()))
        .collect()
}

#[test]
fn finds_a_simple_invocation() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "EPIC_CONFIG(\"osc=hspll, wdt=off\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll, wdt=off"));
}

#[test]
fn returns_none_when_absent() {
    assert_eq!(
        find_epic_config(&src(&[("main.c", "void main(void) {}\n")])),
        None
    );
}

#[test]
fn skips_a_line_comment_that_looks_like_an_invocation() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "// EPIC_CONFIG(\"osc=xt\");\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn skips_a_block_comment_that_looks_like_an_invocation() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "/* EPIC_CONFIG(\"osc=xt\"); */\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn does_not_misparse_a_string_literal_containing_a_comment_delimiter() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "const char *s = \"/* not a comment */\";\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn finds_it_in_any_of_several_files() {
    let found = find_epic_config(&src(&[
        ("a.c", "void from_a(void) {}\n"),
        ("b.c", "EPIC_CONFIG(\"osc=xt\");\n"),
        ("c.c", "void from_c(void) {}\n"),
    ]));
    assert_eq!(found.as_deref(), Some("osc=xt"));
}

#[test]
#[should_panic(expected = "more than one EPIC_CONFIG")]
fn panics_on_more_than_one_invocation_across_the_whole_program() {
    find_epic_config(&src(&[
        ("a.c", "EPIC_CONFIG(\"osc=xt\");\n"),
        ("b.c", "EPIC_CONFIG(\"osc=hspll\");\n"),
    ]));
}
