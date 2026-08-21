use moss_core::ast::{parse, render_document, DefaultHooks};
fn r(m: &str) -> String { render_document(&parse(m), &DefaultHooks::new()) }

#[test]
fn probe() {
    let cases: Vec<(&str, &str)> = vec![
        ("lazy continuation", "a[^1]\n\n[^1]: first line\nsecond line\n"),
        ("4-space continuation", "a[^1]\n\n[^1]: first line\n    second line\n"),
        ("4-space blank then para", "a[^1]\n\n[^1]: first line\n\n    second para\n"),
        ("2-space continuation", "a[^1]\n\n[^1]: first line\n  second line\n"),
        ("blank then unindented", "a[^1]\n\n[^1]: first line\n\nplain para\n"),
        ("indented list", "a[^1]\n\n[^1]: note\n\n    - item\n    - item2\n"),
        ("multi-para lazy", "a[^1]\n\n[^1]: p1\n\n    p2\n\nafter\n"),
    ];
    for (name, md) in cases {
        println!("\n=== {name} ===\n{}\n---> {}", md, r(md));
    }
}
