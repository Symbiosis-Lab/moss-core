use super::*;

#[test]
fn test_parse_with_frontmatter() {
    let input = "---\ntitle: Hello World\ndate: 2024-01-15\n---\nBody content here.";
    let doc = parse(input);

    assert_eq!(doc.frontmatter.len(), 2);
    assert_eq!(
        doc.frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Hello World")
    );
    assert_eq!(
        doc.frontmatter.get("date").and_then(|v| v.as_str()),
        Some("2024-01-15")
    );
    assert_eq!(doc.body, "Body content here.");
    assert!(doc.frontmatter_range.is_some());
    assert!(
        doc.frontmatter_error.is_none(),
        "valid YAML reports no error"
    );
}

#[test]
fn test_parse_no_frontmatter() {
    let input = "Just body content.";
    let doc = parse(input);

    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, "Just body content.");
    assert!(doc.frontmatter_range.is_none());
    assert!(doc.frontmatter_error.is_none(), "no block → no YAML error");
}

#[test]
fn test_parse_empty_frontmatter() {
    let input = "---\n---\nBody after empty frontmatter.";
    let doc = parse(input);

    // serde_yaml::from_str("") returns Err for empty input, so this
    // should be treated as no-frontmatter (invalid YAML).
    // Actually, empty string can produce Null rather than a map.
    // Either way the behavior is graceful.
    assert_eq!(doc.body, "Body after empty frontmatter.");
}

#[test]
fn test_parse_no_closing_delimiter() {
    let input = "---\ntitle: Hello\nno closing";
    let doc = parse(input);

    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, input);
    assert!(doc.frontmatter_range.is_none());
    assert!(
        doc.frontmatter_error.is_none(),
        "unterminated block is not a YAML parse error; body stays whole"
    );
}

#[test]
fn test_parse_yaml_arrays() {
    let input = "---\ntags:\n  - rust\n  - wasm\n---\nBody.";
    let doc = parse(input);

    let tags = doc.frontmatter.get("tags").expect("tags field");
    let seq = tags.as_sequence().expect("should be sequence");
    assert_eq!(seq.len(), 2);
    assert_eq!(seq[0].as_str(), Some("rust"));
    assert_eq!(seq[1].as_str(), Some("wasm"));
}

#[test]
fn test_parse_boolean_values() {
    let input = "---\ndraft: true\n---\nContent.";
    let doc = parse(input);

    assert_eq!(
        doc.frontmatter.get("draft").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_parse_numeric_values() {
    let input = "---\nweight: 42\nrating: 3.5\n---\nContent.";
    let doc = parse(input);

    assert_eq!(
        doc.frontmatter.get("weight").and_then(|v| v.as_u64()),
        Some(42)
    );
    assert_eq!(
        doc.frontmatter.get("rating").and_then(|v| v.as_f64()),
        Some(3.5)
    );
}

#[test]
fn test_parse_preserves_body_exactly() {
    let body = "Line 1\n\nLine 3 with **bold**\n\n- list item\n";
    let input = format!("---\ntitle: Test\n---\n{}", body);
    let doc = parse(&input);

    assert_eq!(doc.body, body);
}

#[test]
fn test_frontmatter_range_byte_offsets() {
    let input = "---\ntitle: Hi\n---\nBody.";
    let doc = parse(input);

    let (start, end) = doc.frontmatter_range.expect("range");
    assert_eq!(start, 0);
    // "---\ntitle: Hi\n---\n" = 18 bytes. The slices below assert the
    // byte-offset contract of `frontmatter_range`: each offset lands on a
    // line boundary (after `\n`), which is ASCII and therefore char-aligned.
    #[allow(clippy::string_slice)] // char-aligned: range returns line-boundary byte offsets
    {
        assert_eq!(&input[start..end], "---\ntitle: Hi\n---\n");
        assert_eq!(&input[end..], "Body.");
    }
}

#[test]
fn test_serialize_with_frontmatter() {
    let mut fm = HashMap::new();
    fm.insert(
        "title".to_string(),
        serde_yaml::Value::String("Hello".to_string()),
    );

    let result = serialize(&fm, "Body content.").expect("serialize");

    assert!(result.starts_with("---\n"));
    assert!(result.contains("title: Hello"));
    assert!(result.contains("---\nBody content."));
}

#[test]
fn test_serialize_empty_frontmatter() {
    let fm = HashMap::new();
    let result = serialize(&fm, "Just body.").expect("serialize");
    assert_eq!(result, "Just body.");
}

#[test]
fn test_parse_invalid_yaml() {
    let input = "---\n: invalid: yaml: [unclosed\n---\nBody.";
    let doc = parse(input);

    // Invalid YAML should fall back to no-frontmatter, but now the block is
    // recorded, the error surfaced, and the body kept whole (no data loss).
    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, input, "body preserved whole on YAML error");
    assert!(doc.frontmatter_error.is_some());
    assert!(doc.frontmatter_range.is_some());
    assert_eq!(
        doc.render_body(),
        "Body.",
        "render view excludes the bad block"
    );
}

#[test]
fn test_parse_frontmatter_with_trailing_whitespace_on_delimiter() {
    let input = "---\ntitle: Test\n---  \nBody.";
    let doc = parse(input);

    // The closing delimiter has trailing spaces — `line.trim() == "---"` should match.
    assert_eq!(
        doc.frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Test")
    );
    assert_eq!(doc.body, "Body.");
}

#[test]
fn test_parse_content_starts_with_dashes_but_not_frontmatter() {
    let input = "---- Not frontmatter\nJust text.";
    let doc = parse(input);

    // Starts with "----" (4 dashes), which starts_with("---") is true.
    // But after the first line, there's no closing `---`.
    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, input);
}

#[test]
fn test_roundtrip() {
    let input = "---\ntitle: Round Trip\n---\nBody stays the same.";
    let doc = parse(input);

    let output = serialize(&doc.frontmatter, &doc.body).expect("serialize");

    // Re-parse and verify
    let doc2 = parse(&output);
    assert_eq!(doc.frontmatter.get("title"), doc2.frontmatter.get("title"));
    assert_eq!(doc.body, doc2.body);
}

#[test]
fn test_parse_multiline_body() {
    let input = "---\ntitle: Test\n---\nParagraph 1.\n\nParagraph 2.\n\n> Quote\n";
    let doc = parse(input);

    assert_eq!(doc.body, "Paragraph 1.\n\nParagraph 2.\n\n> Quote\n");
}

#[test]
fn test_parse_only_dashes() {
    let input = "---";
    let doc = parse(input);

    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, "---");
}

#[test]
fn test_parse_crlf_content() {
    let input = "---\r\ntitle: Hello World\r\ndate: 2024-01-15\r\n---\r\nBody content here.";
    let doc = parse(input);

    assert_eq!(doc.frontmatter.len(), 2);
    assert_eq!(
        doc.frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Hello World")
    );
    assert_eq!(
        doc.frontmatter.get("date").and_then(|v| v.as_str()),
        Some("2024-01-15")
    );
    assert_eq!(doc.body, "Body content here.");
    assert!(doc.frontmatter_range.is_some());
}

#[test]
fn test_parse_crlf_byte_offsets() {
    let input = "---\r\ntitle: Hi\r\n---\r\nBody.";
    let doc = parse(input);

    let (start, end) = doc.frontmatter_range.expect("range");
    assert_eq!(start, 0);
    // After CRLF normalization, offsets are relative to the normalized string.
    // "---\ntitle: Hi\n---\n" = 18 bytes
    assert_eq!(end, 18);
}

#[test]
fn test_parse_crlf_preserves_body() {
    let body = "Line 1\nLine 2\n";
    let input = format!(
        "---\r\ntitle: Test\r\n---\r\n{}",
        body.replace('\n', "\r\n")
    );
    let doc = parse(&input);

    assert_eq!(
        doc.frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Test")
    );
    // Body CRLF is also normalized to LF.
    assert_eq!(doc.body, body);
}

#[test]
fn test_parse_crlf_yaml_arrays() {
    let input = "---\r\ntags:\r\n  - rust\r\n  - wasm\r\n---\r\nBody.";
    let doc = parse(input);

    let tags = doc.frontmatter.get("tags").expect("tags field");
    let seq = tags.as_sequence().expect("should be sequence");
    assert_eq!(seq.len(), 2);
    assert_eq!(seq[0].as_str(), Some("rust"));
    assert_eq!(seq[1].as_str(), Some("wasm"));
}

#[test]
fn test_uid_scientific_notation_roundtrip() {
    // Regression test: UIDs like "753659e7" look like YAML scientific
    // notation and get parsed as floats. The serialize path must quote them.
    let input = "---\ntitle: Test\nuid: \"753659e7\"\n---\nBody.";
    let doc = parse(input);

    // When properly quoted, uid is parsed as a string
    let uid_val = doc.frontmatter.get("uid").expect("uid field");
    assert_eq!(uid_val.as_str(), Some("753659e7"));

    // Round-trip: serialize and re-parse
    let output = serialize(&doc.frontmatter, &doc.body).expect("serialize");
    let doc2 = parse(&output);
    let uid2 = doc2
        .frontmatter
        .get("uid")
        .expect("uid field after roundtrip");
    assert_eq!(uid2.as_str(), Some("753659e7"));
}

#[test]
fn test_value_as_string_handles_numbers() {
    // If a uid was already corrupted to a number by YAML parsing,
    // value_as_string should still extract a usable string.
    let num_val = serde_yaml::Value::Number(serde_yaml::Number::from(75365900));
    assert!(value_as_string(&num_val).is_some());

    let str_val = serde_yaml::Value::String("753659e7".to_string());
    assert_eq!(value_as_string(&str_val), Some("753659e7".to_string()));
}

#[test]
fn test_unquoted_uid_parsed_as_number() {
    // Demonstrates the bug: unquoted hex-like UIDs are parsed as numbers
    let input = "---\ntitle: Test\nuid: 753659e7\n---\nBody.";
    let doc = parse(input);

    let uid_val = doc.frontmatter.get("uid").expect("uid field");
    // serde_yaml parses this as a number, not a string
    assert!(
        uid_val.as_str().is_none(),
        "Unquoted 753659e7 should NOT parse as string (it's a YAML number)"
    );

    // But value_as_string can still extract it
    assert!(value_as_string(uid_val).is_some());
}

#[test]
fn test_serialize_strips_stray_control_chars() {
    // Regression test for the macOS Tauri multiwebview arrow-key bug
    // (tauri-apps/tauri#10194): a child webview's beforeinput/keyDown
    // path can insert the arrow key's legacy control code (Right =
    // U+001D GROUP SEPARATOR) into a plain input instead of just moving
    // the caret. The frontend guards this at `beforeinput`
    // (frontend/app/ui/control-char-guard.ts); this write-boundary strip
    // is the defense-in-depth backstop so a corrupted value can never
    // reach disk even if it slips past the DOM guard.
    let corrupted = format!("websites.{}", "\u{1D}".repeat(8));

    let mut fm = HashMap::new();
    fm.insert(
        "description".to_string(),
        serde_yaml::Value::String(corrupted),
    );

    let output = serialize(&fm, "Body.").expect("serialize");
    let doc = parse(&output);

    assert_eq!(
        doc.frontmatter.get("description").and_then(|v| v.as_str()),
        Some("websites."),
        "control chars must be stripped from the written value"
    );
}

/// THE anti-regression test for the "malformed frontmatter leaks verbatim"
/// bug (William Blake "Europe - A Prophecy.md"). Two YAML keys collapsed onto
/// one line (`uid: blk-europecover: "006.jpg"`) make serde_yaml fail. The
/// parser must (a) report the error, (b) keep the WHOLE document as `body`
/// (no data loss — the editor must still see and be able to repair the block),
/// and (c) record the block's byte range.
#[test]
fn test_parse_invalid_yaml_preserves_body_no_data_loss() {
    let input =
            "---\nchildren_style: grid\nseries: true\nweight: 10\nuid: blk-europecover: \"006.jpg\"\n---\n\n\ngh\n![[x.jpg]]\n";
    let doc = parse(input);

    assert!(
        doc.frontmatter.is_empty(),
        "malformed YAML yields no fields"
    );
    assert!(
        doc.frontmatter_error.is_some(),
        "the serde_yaml error must be surfaced, not swallowed"
    );
    // Range covers the delimited block; body is the WHOLE document (block NOT
    // trimmed) so the editor can still show/repair it and a re-serialize save
    // preserves the file.
    let (start, fm_end) = doc.frontmatter_range.expect("range on malformed block");
    assert_eq!(start, 0);
    assert_eq!(
        doc.body, input,
        "body must be the whole document — no data loss"
    );
    #[allow(clippy::string_slice)] // line-boundary offsets, char-aligned
    {
        assert!(
            input[0..fm_end].starts_with("---\n") && input[0..fm_end].ends_with("---\n"),
            "frontmatter_range must bound the `---...---\\n` block"
        );
    }
}

/// The build-facing render view excludes a failed block so malformed YAML
/// never leaks verbatim into published HTML.
#[test]
fn test_render_body_excludes_failed_block() {
    let input =
            "---\nchildren_style: grid\nseries: true\nweight: 10\nuid: blk-europecover: \"006.jpg\"\n---\n\n\ngh\n![[x.jpg]]\n";
    let doc = parse(input);

    let rendered = doc.render_body();
    assert!(
        !rendered.contains("---"),
        "delimiters must not leak: {rendered:?}"
    );
    assert!(
        !rendered.contains("uid:"),
        "raw YAML must not leak: {rendered:?}"
    );
    assert_eq!(
        rendered, "\n\ngh\n![[x.jpg]]\n",
        "render_body is exactly the content after the closing delimiter"
    );
}

/// On success (and no-frontmatter), render_body is a no-op equal to body.
#[test]
fn test_render_body_equals_body_on_success() {
    let ok = parse("---\ntitle: Hi\n---\nBody.");
    assert!(ok.frontmatter_error.is_none());
    assert_eq!(ok.render_body(), ok.body);
    assert_eq!(ok.render_body(), "Body.");

    let none = parse("No frontmatter here.");
    assert!(none.frontmatter_error.is_none());
    assert_eq!(none.render_body(), none.body);
}

#[test]
fn test_strip_control_chars_str_keeps_tab_lf_cr() {
    // TAB/LF/CR are legitimate whitespace and must survive the strip —
    // mirrors the frontend guard's CONTROL_RANGES exclusions.
    let input = "a\tb\nc\rd\u{00}\u{7f}\u{85}e";
    assert_eq!(strip_control_chars_str(input), "a\tb\nc\rde");
}

// ── Structural asset spans in frontmatter (2026-08-03) ───────────────────

use crate::resolve::md_extract::PathContainer;

fn fm_spans(src: &str) -> Vec<crate::resolve::md_extract::AssetPathSpan> {
    frontmatter_asset_spans(src)
}

#[test]
fn frontmatter_cover_bare() {
    let src = "---\ntitle: Hi\ncover: 關於/x.png\n---\n\nBody\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(&src[s[0].value.clone()], "關於/x.png");
    assert_eq!(s[0].path, "關於/x.png");
    assert_eq!(s[0].quote, None);
    assert_eq!(
        s[0].container,
        PathContainer::FrontmatterField { key: "cover".into() }
    );
    assert_eq!(&src[s[0].outer.clone()], "cover: 關於/x.png\n");
}

#[test]
fn frontmatter_cover_double_quoted() {
    let src = "---\ncover: \"my photo.png\"\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(&src[s[0].value.clone()], "\"my photo.png\"");
    assert_eq!(s[0].path, "my photo.png");
    assert_eq!(s[0].quote, Some('"'));
}

#[test]
fn frontmatter_cover_single_quoted() {
    let src = "---\ncover: 'x.png'\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(&src[s[0].value.clone()], "'x.png'");
    assert_eq!(s[0].path, "x.png");
    assert_eq!(s[0].quote, Some('\''));
}

#[test]
fn frontmatter_cover_with_pipe_attrs() {
    let src = "---\ncover: x.png|cover top\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].path, "x.png");
    assert_eq!(s[0].attrs, "cover top");
}

#[test]
fn frontmatter_cover_with_trailing_comment() {
    let src = "---\ncover: x.png  # keep this\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(
        &src[s[0].value.clone()],
        "x.png",
        "the span must stop before the ` #` comment"
    );
}

#[test]
fn frontmatter_wikilink_cover_path_matches_generic() {
    // `cover: '[[DSCF.jpeg]]'` must yield the same path the generic token
    // scanner yields for the same bytes, or the rename/delete edit ordering
    // in ref_scan resolves them differently.
    let src = "---\ncover: '[[DSCF.jpeg]]'\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].path, "DSCF.jpeg");
    let generic = crate::resolve::md_extract::extract_md_references(src);
    assert_eq!(generic.len(), 1);
    assert_eq!(generic[0].text, "DSCF.jpeg");
}

#[test]
fn frontmatter_block_scalar_body_is_not_scanned() {
    let src = "---\ndescription: |\n  cover: not-a-field.png\n  more text\ncover: real.png\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].path, "real.png");
}

#[test]
fn nested_cover_after_blank_line_is_found() {
    // Why frontmatter is scanned on the RAW source: this 4-space-indented
    // key after a blank line reads as an indented code block to
    // `inert_regions` and would be silently dropped.
    let src = "---\ncascade:\n\n    cover: nested.png\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].path, "nested.png");
}

#[test]
fn frontmatter_logo_uses_schema_derived_field_set() {
    let src = "---\nlogo: brand.svg\nunrelated: other.png\n---\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "only schema FilePicker fields: {s:?}");
    assert_eq!(s[0].path, "brand.svg");
}

#[test]
fn frontmatter_flow_collection_is_skipped() {
    let src = "---\ncover: [a.png, b.png]\n---\n";
    assert!(fm_spans(src).is_empty());
}

#[test]
fn no_frontmatter_delimiters_yields_no_frontmatter_refs() {
    assert!(fm_spans("cover: x.png\n\nBody\n").is_empty());
    assert!(fm_spans("---\ncover: x.png\n").is_empty(), "unclosed block");
}

#[test]
fn crlf_frontmatter_spans_are_exact() {
    let src = "---\r\ncover: x.png\r\n---\r\n";
    let s = fm_spans(src);
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(&src[s[0].value.clone()], "x.png");
    assert_eq!(&src[s[0].outer.clone()], "cover: x.png\r\n");
}
