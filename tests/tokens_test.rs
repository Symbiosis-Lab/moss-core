//! Tests for the tokens loader.

use moss_core::contract::tokens::load_tokens;

#[test]
fn load_tokens_parses_w3c_format() {
    let tokens = load_tokens().expect("tokens.json must parse");

    // Top-level groups are present in source order (from $order field)
    let group_names: Vec<&str> = tokens.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(group_names, vec!["typography", "color", "layout", "spacing"]);
}

#[test]
fn color_group_has_accent_token() {
    let tokens = load_tokens().expect("tokens.json must parse");
    let color = tokens.groups.iter().find(|g| g.name == "color")
        .expect("color group must exist");

    let accent = color.entries.iter().find(|t| t.name == "moss-color-accent")
        .expect("moss-color-accent must exist");
    assert_eq!(accent.value, "#2d5a2d");
    assert_eq!(accent.type_hint.as_deref(), Some("color"));
}

#[test]
fn entries_are_sorted_alphabetically_within_group() {
    let tokens = load_tokens().expect("tokens.json must parse");
    let color = tokens.groups.iter().find(|g| g.name == "color")
        .expect("color group must exist");

    let names: Vec<&str> = color.entries.iter().map(|t| t.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "entries within a group must be alphabetical");
}

#[test]
fn token_value_preserves_var_references() {
    let tokens = load_tokens().expect("tokens.json must parse");
    let layout = tokens.groups.iter().find(|g| g.name == "layout")
        .expect("layout group must exist");

    let nav_width = layout.entries.iter().find(|t| t.name == "moss-nav-width")
        .expect("moss-nav-width must exist");
    assert_eq!(nav_width.value, "var(--moss-content-width)");
}

// Error-path tests using parse_tokens helper
use moss_core::contract::tokens::parse_tokens;

#[test]
fn parse_tokens_errors_when_order_missing() {
    let input = "{\n  \"color\": {\n    \"moss-color-accent\": {\"$type\": \"color\", \"$value\": \"#000\"}\n  }\n}";
    let err = parse_tokens(input).expect_err("must fail");
    assert!(err.contains("$order"), "error should mention $order: {}", err);
}

#[test]
fn parse_tokens_errors_when_group_named_in_order_is_missing() {
    let input = "{\n  \"$order\": [\"color\", \"spacing\"],\n  \"color\": {\n    \"moss-color-accent\": {\"$type\": \"color\", \"$value\": \"#000\"}\n  }\n}";
    let err = parse_tokens(input).expect_err("must fail");
    assert!(err.contains("spacing"), "error should mention missing group: {}", err);
}

#[test]
fn parse_tokens_errors_when_entry_missing_value() {
    let input = "{\n  \"$order\": [\"color\"],\n  \"color\": {\n    \"moss-color-accent\": {\"$type\": \"color\"}\n  }\n}";
    let err = parse_tokens(input).expect_err("must fail");
    assert!(err.contains("$value"), "error should mention missing $value: {}", err);
}
