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

use driver::prescan::{find_epic_configs, find_pragma_config, pragma_spec};

fn pragma_in(text: &str) -> Vec<driver::prescan::PragmaSetting> {
    find_pragma_config(&src(&[("main.c", text)]))
}

#[test]
fn recovers_a_simple_pragma_with_its_site() {
    let found = pragma_in("#pragma config FOSC = HS\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "FOSC");
    assert_eq!(found[0].value, "HS");
    assert_eq!((found[0].line, found[0].col), (1, 1));
}

#[test]
fn recovers_comma_pairs_indented_and_mixed_case() {
    let found = pragma_in("  #PRAGMA CONFIG osc=xt, WDT = off\n");
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].name.as_str(), found[0].value.as_str()),
        ("osc", "xt")
    );
    assert_eq!(
        (found[1].name.as_str(), found[1].value.as_str()),
        ("WDT", "off")
    );
    assert_eq!((found[0].line, found[0].col), (1, 3));
}

#[test]
fn recovers_pragmas_across_files() {
    let found = find_pragma_config(&src(&[
        ("a.c", "#pragma config FOSC = HS\n"),
        ("b.c", "void f(void) {}\n#pragma config WDTE = OFF\n"),
    ]));
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].file, "b.c");
    assert_eq!((found[1].line, found[1].col), (2, 1));
}

#[test]
fn skips_comments_strings_and_other_pragmas() {
    let found = pragma_in(
        "// #pragma config FOSC = XT\n\
         /* #pragma config FOSC = XT */\n\
         const char *s = \"#pragma config FOSC = XT\";\n\
         #pragma once\n\
         #pragma config FOSC = HS\n",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].value, "HS");
    assert_eq!(found[0].line, 5);
}

#[test]
fn epic_hits_carry_their_site() {
    let found = find_epic_configs(&src(&[("m.c", "\nEPIC_CONFIG(\"osc=xt\");\n")]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].spec, "osc=xt");
    assert_eq!((found[0].line, found[0].col), (2, 1));
}

#[test]
#[should_panic(expected = "malformed #pragma config")]
fn panics_on_a_pragma_without_a_value() {
    pragma_in("#pragma config FOSC\n");
}

#[test]
fn skips_hash_lines_that_are_not_config_pragmas() {
    let found = pragma_in(
        "#include <stdint.h>\n\
         #define FOO 1\n\
         #pragma warning disable 123\n\
         #pragma config FOSC = HS\n",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "FOSC");
}

#[test]
fn pragma_spec_joins_validated_pairs() {
    let spec = pragma_spec(
        &device::PIC16F877A.config,
        &find_pragma_config(&src(&[("m.c", "#pragma config FOSC = XT, WDTE = OFF\n")])),
    );
    assert_eq!(spec, "FOSC=XT, WDTE=OFF");
    assert_eq!(
        device::resolve_config(&device::PIC16F877A.config, &spec),
        device::resolve_config(&device::PIC16F877A.config, "osc=xt, wdt=off")
    );
}

#[test]
#[should_panic(expected = "unknown config field 'WAT'")]
fn pragma_spec_names_valid_fields() {
    let err = std::panic::catch_unwind(|| {
        pragma_spec(
            &device::PIC16F877A.config,
            &find_pragma_config(&src(&[("m.c", "#pragma config WAT = OFF\n")])),
        )
    })
    .unwrap_err();
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("expected one of:"), "{msg}");
    assert!(msg.contains("osc"), "{msg}");
    panic!("{msg}");
}

#[test]
#[should_panic(expected = "unknown config value 'TURBO'")]
fn pragma_spec_names_valid_values() {
    pragma_spec(
        &device::PIC16F877A.config,
        &find_pragma_config(&src(&[("m.c", "#pragma config FOSC = TURBO\n")])),
    );
}

#[test]
#[should_panic(expected = "conflicting #pragma config")]
fn pragma_spec_rejects_conflicting_duplicates() {
    pragma_spec(
        &device::PIC16F877A.config,
        &find_pragma_config(&src(&[(
            "m.c",
            "#pragma config FOSC = XT\n#pragma config FOSC = HS\n",
        )])),
    );
}

#[test]
fn pragma_spec_accepts_repeated_equal_values() {
    let spec = pragma_spec(
        &device::PIC16F877A.config,
        &find_pragma_config(&src(&[(
            "m.c",
            "#pragma config FOSC = XT\n#pragma config FOSC = XT\n",
        )])),
    );
    assert_eq!(spec, "FOSC=XT");
}

#[test]
fn accepts_a_trailing_comment_after_pairs() {
    let found = pragma_in(
        "#pragma config FOSC = HS // crystal\n#pragma config WDTE = OFF /* watchdog */\n",
    );
    assert_eq!(found.len(), 2);
}

#[test]
#[should_panic(expected = "malformed #pragma config")]
fn panics_on_trailing_garbage_after_pairs() {
    pragma_in("#pragma config FOSC = HS junk\n");
}

#[test]
fn accepts_whitespace_between_hash_and_pragma() {
    let found = pragma_in("#  pragma   config   FOSC = HS\n# pragma config WDTE=OFF\n");
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].name.as_str(), found[1].name.as_str()),
        ("FOSC", "WDTE")
    );
}

#[test]
fn finds_a_plain_xtal_freq_with_its_site() {
    let found = driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "#include <xc.h>\n#define _XTAL_FREQ 4000000\n",
    )]))
    .unwrap();
    assert_eq!(found.value, Some(4_000_000));
    assert_eq!((found.line, found.col), (2, 1));
}

#[test]
fn xtal_freq_accepts_hex_suffixes_and_parens() {
    for (body, want) in [
        ("#define _XTAL_FREQ 0x3D0900\n", 4_000_000),
        ("#define _XTAL_FREQ 4000000UL\n", 4_000_000),
        ("#define _XTAL_FREQ (8000000)\n", 8_000_000),
    ] {
        let found = driver::prescan::find_xtal_freq(&src(&[("main.c", body)])).unwrap();
        assert_eq!(found.value, Some(want), "{body}");
    }
}

#[test]
fn xtal_freq_last_definition_wins_and_comments_do_not_count() {
    let found = driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "// #define _XTAL_FREQ 1000000\n#define _XTAL_FREQ 4000000\n#define _XTAL_FREQ 8000000\n",
    )]))
    .unwrap();
    assert_eq!(found.value, Some(8_000_000));
    assert_eq!(found.line, 3);
}

#[test]
fn xtal_freq_computed_definition_is_present_but_unknown() {
    let found =
        driver::prescan::find_xtal_freq(&src(&[("main.c", "#define _XTAL_FREQ (8000000/2)\n")]))
            .unwrap();
    assert_eq!(found.value, None);
}

#[test]
fn finds_a_delay_call_but_not_a_comment_or_its_name_alone() {
    let hit = driver::prescan::find_delay_use(&src(&[(
        "main.c",
        "// __delay_ms(1);\nvoid main(void) { __delay_us (10); }\n",
    )]))
    .unwrap();
    assert_eq!(hit.name, "__delay_us");
    assert_eq!(hit.line, 2);
    assert!(driver::prescan::find_delay_use(&src(&[("main.c", "int __delay_ms;\n")])).is_none());
}

#[test]
fn xtal_freq_ignores_a_trailing_comment() {
    let found = driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "#define _XTAL_FREQ 4000000 // 4 MHz\n",
    )]))
    .unwrap();
    assert_eq!(found.value, Some(4_000_000));
}

#[test]
fn xtal_freq_ignores_a_function_like_macro() {
    assert!(driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "#define _XTAL_FREQ(x) ((x) * 1000)\n"
    )]))
    .is_none());
}

#[test]
fn xtal_freq_undef_clears_the_last_definition() {
    assert!(driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "#define _XTAL_FREQ 4000000\n#undef _XTAL_FREQ\n"
    )]))
    .is_none());
    let found = driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "#define _XTAL_FREQ 4000000\n#undef _XTAL_FREQ\n#define _XTAL_FREQ 8000000\n",
    )]))
    .unwrap();
    assert_eq!(found.value, Some(8_000_000));
}

#[test]
fn delay_use_ignores_directive_lines() {
    assert!(driver::prescan::find_delay_use(&src(&[(
        "main.c",
        "#ifdef __delay_ms\n#endif\nvoid main(void) {}\n"
    )]))
    .is_none());
}

#[test]
fn finds_epic_config_after_a_char_literal_holding_a_quote() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "char q = '\"';\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn finds_pragma_config_after_a_char_literal_holding_a_quote() {
    let found = pragma_in("char q = '\"';\n#pragma config FOSC = HS\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].value, "HS");
    assert_eq!(found[0].line, 2);
}

#[test]
fn finds_xtal_freq_after_a_char_literal_holding_a_quote() {
    let found = driver::prescan::find_xtal_freq(&src(&[(
        "main.c",
        "char q = '\"';\n#define _XTAL_FREQ 4000000\n",
    )]))
    .unwrap();
    assert_eq!(found.value, Some(4_000_000));
}

#[test]
fn finds_delay_use_after_a_char_literal_holding_a_quote() {
    let hit = driver::prescan::find_delay_use(&src(&[(
        "main.c",
        "char q = '\"';\nvoid main(void) { __delay_ms(10); }\n",
    )]))
    .unwrap();
    assert_eq!(hit.name, "__delay_ms");
    assert_eq!(hit.line, 2);
}

#[test]
fn skips_an_escaped_quote_inside_a_char_literal() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "char q = '\\'';\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn an_unterminated_quote_does_not_swallow_the_next_line() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "char q = ';\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn xtal_freq_still_accepts_digit_separators() {
    let found =
        driver::prescan::find_xtal_freq(&src(&[("main.c", "#define _XTAL_FREQ 4'000'000\n")]))
            .unwrap();
    assert_eq!(found.value, Some(4_000_000));
}
