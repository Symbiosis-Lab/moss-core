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
    // A token whose $value is a var() reference must round-trip verbatim
    // (no resolution/inlining at load time). moss-reading-size aliases the base
    // size the same way; it replaced moss-nav-width as the example here when
    // nav-width became an opt-in (unset-by-default) escape hatch — see the
    // .main-nav fallback in site.css.
    let tokens = load_tokens().expect("tokens.json must parse");
    let typography = tokens.groups.iter().find(|g| g.name == "typography")
        .expect("typography group must exist");

    let reading_size = typography.entries.iter().find(|t| t.name == "moss-reading-size")
        .expect("moss-reading-size must exist");
    assert_eq!(reading_size.value, "var(--moss-reading-size-base)");
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

use moss_core::contract::tokens::format_root_block;

#[test]
fn format_root_block_produces_expected_shape() {
    let tokens = load_tokens().expect("tokens.json must parse");
    let css = format_root_block(&tokens);

    // Group comments appear
    assert!(css.contains("/* Typography */"));
    assert!(css.contains("/* Color */"));
    assert!(css.contains("/* Layout */"));
    assert!(css.contains("/* Spacing */"));

    // Tokens are present
    assert!(css.contains("--moss-color-accent: #2d5a2d;"));
    assert!(css.contains("--moss-content-width: 67ch;"));
    assert!(css.contains("--moss-space-xs: 0.5rem;"));

    // Two-space indent
    assert!(css.contains("\n  --moss-color-accent"));

    // Group order matches tokens.json source order
    let typo_idx = css.find("/* Typography */").unwrap();
    let color_idx = css.find("/* Color */").unwrap();
    let layout_idx = css.find("/* Layout */").unwrap();
    let spacing_idx = css.find("/* Spacing */").unwrap();
    assert!(typo_idx < color_idx);
    assert!(color_idx < layout_idx);
    assert!(layout_idx < spacing_idx);

    // Alphabetical within group: accent before bg before muted before surface before text
    let accent_idx = css.find("--moss-color-accent").unwrap();
    let bg_idx = css.find("--moss-color-bg").unwrap();
    let muted_idx = css.find("--moss-color-muted").unwrap();
    let surface_idx = css.find("--moss-color-surface").unwrap();
    let text_idx = css.find("--moss-color-text").unwrap();
    assert!(accent_idx < bg_idx);
    assert!(bg_idx < muted_idx);
    assert!(muted_idx < surface_idx);
    assert!(surface_idx < text_idx);

    // Wrapped in :root { ... }
    assert!(css.starts_with(":root {\n"));
    assert!(css.trim_end().ends_with("}"));
}

#[test]
fn format_root_block_normalizes_colors_to_lowercase_hex() {
    // tokens.json should already have lowercase hex, but the formatter
    // is the layer that enforces the rule. Verify all hex values in output
    // are lowercase 6-digit.
    let tokens = load_tokens().expect("tokens.json must parse");
    let css = format_root_block(&tokens);

    for line in css.lines() {
        if let Some(hash_idx) = line.find('#') {
            let after = &line[hash_idx + 1..];
            let hex_part: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            assert!(
                !hex_part.chars().any(|c| c.is_ascii_uppercase()),
                "found uppercase hex in: {}",
                line
            );
        }
    }
}

#[test]
fn format_root_block_normalizes_3digit_hex_to_6digit() {
    // Direct unit test on the helper (via the public format function).
    // tokens.json doesn't currently use 3-digit hex; this asserts the
    // expansion behavior in case the source ever changes.
    use moss_core::contract::tokens::{Tokens, TokenGroup, TokenEntry, format_root_block};

    let tokens = Tokens {
        groups: vec![TokenGroup {
            name: "color".to_string(),
            description: None,
            entries: vec![TokenEntry {
                name: "test-color".to_string(),
                value: "#FFF".to_string(),
                type_hint: Some("color".to_string()),
                description: None,
            }],
        }],
    };
    let css = format_root_block(&tokens);
    assert!(css.contains("--test-color: #ffffff;"), "got: {}", css);
}
