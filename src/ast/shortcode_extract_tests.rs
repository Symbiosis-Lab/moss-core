use super::*;

#[test]
fn no_shortcodes_round_trips_input() {
    let md = "# Heading\n\npara with [link](u).\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.markdown_with_placeholders, md);
    assert!(result.extracted.is_empty());
}

#[test]
fn extracts_subscribe_block_with_placeholder_and_button_attrs() {
    let md = r#":::subscribe {placeholder="you@domain.com" button="Sign me up"}
:::
"#;
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(args) => {
            assert_eq!(args.placeholder.as_deref(), Some("you@domain.com"));
            assert_eq!(args.button.as_deref(), Some("Sign me up"));
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
    assert!(result
        .markdown_with_placeholders
        .contains(&placeholder_for(&result.nonce, 0)));
    assert!(!result.markdown_with_placeholders.contains(":::subscribe"));
}

#[test]
fn extracts_subscribe_block_with_only_placeholder_attr() {
    let md = r#":::subscribe {placeholder="hi@example.com"}
:::
"#;
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(args) => {
            assert_eq!(args.placeholder.as_deref(), Some("hi@example.com"));
            assert!(args.button.is_none());
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn extracts_subscribe_block_with_no_args() {
    let md = ":::subscribe\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(args) => {
            assert!(args.placeholder.is_none());
            assert!(args.button.is_none());
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn extracts_subscribe_block_with_multi_line_attrs() {
    let md = r#":::subscribe {
  placeholder="you@domain.com"
  button="Request access"
}
:::
"#;
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(args) => {
            assert_eq!(args.placeholder.as_deref(), Some("you@domain.com"));
            assert_eq!(args.button.as_deref(), Some("Request access"));
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn subscribe_legacy_body_keys_no_longer_parsed() {
    // The pre-grammar form `description: ...` / `button: ...` body
    // lines are no longer recognized. moss-releases content is
    // rewritten in Step 3; this test pins the post-cut behavior.
    let md = ":::subscribe\ndescription: Get updates\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(args) => {
            assert!(
                args.placeholder.is_none(),
                "old description body must not populate placeholder"
            );
            assert!(args.button.is_none());
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn subscribe_inside_code_fence_is_not_extracted() {
    // Adversarial: `:::subscribe` inside a fenced code block is just
    // documentation text. The extractor must not treat it as a
    // shortcode.
    let md = "```\n:::subscribe\ndescription: doc\n:::\n```\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty());
    assert!(result.markdown_with_placeholders.contains(":::subscribe"));
}

#[test]
fn subscribe_inside_tilde_fence_is_not_extracted() {
    let md = "~~~\n:::subscribe\n:::\n~~~\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty());
}

#[test]
fn unclosed_subscribe_block_emits_verbatim() {
    // Unclosed: emit source verbatim so the author sees the typo.
    let md = ":::subscribe\nbutton: Go\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty());
    assert!(result.markdown_with_placeholders.contains(":::subscribe"));
}

#[test]
fn extracts_hero_block_with_body_image_typed() {
    // Step 2: :::hero is now a typed variant. The extractor consumes
    // it and produces Shortcode::Hero with the body-image fallback
    // populating args.image when no `image=` attribute is present.
    let md = ":::hero\n![[bg.jpg]]\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => match &args.image {
            Some(Url::Unresolved(s)) => assert_eq!(s, "bg.jpg"),
            _ => panic!("expected Unresolved bg.jpg"),
        },
        _ => panic!("expected Hero"),
    }
    // The literal `:::hero` should be replaced by a sentinel.
    assert!(!result.markdown_with_placeholders.contains(":::hero"));
}

#[test]
fn extracts_multiple_subscribes_with_increasing_indices() {
    let md = ":::subscribe\ndescription: a\n:::\n\nsome text\n\n:::subscribe\nbutton: b\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 2);
    assert_eq!(result.extracted[0].index, 0);
    assert_eq!(result.extracted[1].index, 1);
    assert!(result
        .markdown_with_placeholders
        .contains(&placeholder_for(&result.nonce, 0)));
    assert!(result
        .markdown_with_placeholders
        .contains(&placeholder_for(&result.nonce, 1)));
}

#[test]
fn parse_placeholder_round_trips_index() {
    let nonce = "deadbeef";
    for index in [0, 1, 5, 99] {
        let s = placeholder_for(nonce, index);
        assert_eq!(parse_placeholder(nonce, &s), Some(index));
    }
}

#[test]
fn parse_placeholder_rejects_non_placeholder_html() {
    let nonce = "deadbeef";
    assert!(parse_placeholder(nonce, "<div>hi</div>").is_none());
    assert!(parse_placeholder(nonce, "<!--just a comment-->").is_none());
}

#[test]
fn parse_placeholder_rejects_wrong_nonce() {
    // Authored content collision case: an author writes a literal
    // <!--MOSS_SC_*_0--> in their markdown. parse_placeholder requires
    // the same nonce as this extraction's session, so a mismatched
    // nonce returns None. This forecloses the authored-content
    // namespace collision.
    let s = placeholder_for("aaaa1111", 5);
    assert_eq!(parse_placeholder("bbbb2222", &s), None);
}

#[test]
fn extract_uses_content_derived_nonce() {
    // The nonce is deterministic per input — calling extract_shortcodes
    // twice on the same input produces the same nonce.
    let md = ":::subscribe\n:::\n";
    let r1 = extract_shortcodes(md);
    let r2 = extract_shortcodes(md);
    assert_eq!(r1.nonce, r2.nonce);
    // Different inputs produce different nonces (with overwhelming
    // probability — collision impossible to exhibit here).
    let r3 = extract_shortcodes(":::subscribe\ndescription: x\n:::\n");
    assert_ne!(r1.nonce, r3.nonce);
}

#[test]
fn nonce_makes_authored_collision_inert() {
    // If an author writes a literal placeholder-shape comment, my
    // nonce will differ from theirs, so the substitution leaves
    // their text alone.
    let md = ":::subscribe\n:::\n\nLook: <!--MOSS_SC_00000000_0-->\n";
    let result = extract_shortcodes(md);
    // The author's comment survives because the embedded nonce
    // differs from the computed one (probability of collision = 1/2^32).
    assert_ne!(result.nonce, "00000000");
    assert!(result
        .markdown_with_placeholders
        .contains("MOSS_SC_00000000_0"));
}

#[test]
fn parse_shortcode_opener_recognizes_simple_name() {
    assert_eq!(
        parse_shortcode_opener(":::subscribe"),
        Some((3, "subscribe", ""))
    );
}

#[test]
fn parse_shortcode_opener_extracts_args() {
    assert_eq!(
        parse_shortcode_opener(":::grid 3 1:2:1"),
        Some((3, "grid", "3 1:2:1"))
    );
}

#[test]
fn parse_shortcode_opener_recognizes_quadruple_colon() {
    // ::::buttons is the standard way to nest a shortcode inside
    // a :::grid cell. The arity is preserved so the closer matches.
    assert_eq!(
        parse_shortcode_opener("::::buttons"),
        Some((4, "buttons", ""))
    );
}

#[test]
fn parse_shortcode_opener_rejects_two_colons() {
    // Two colons is not a fence.
    assert!(parse_shortcode_opener("::name").is_none());
}

#[test]
fn extracts_quadruple_colon_buttons() {
    // ::::buttons inside hypothetical grid context. We just test the
    // extractor in isolation; grid integration lands in Task 11.
    let md = "::::buttons\n[Tickets](go/)\n::::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 1);
            assert_eq!(args.items[0].text, "Tickets");
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extracts_grid_with_nested_buttons_via_arity() {
    // SoCiviC pattern: `::::buttons` (4-colon) nested inside `:::grid`
    // (3-colon). Phase 4 PR4.5 (2026-05-28) promoted cells from raw
    // markdown strings to `Vec<Vec<Block>>`. The inner `::::buttons`
    // now extracts into a typed `Block::Shortcode(Buttons)` inside the
    // cell at parse time (via the recursive `super::parser::parse` call
    // in `parse_cell_to_blocks`), not at render time.
    let md = ":::grid 2\n::::buttons\n[Tickets](go/)\n::::\n+++\nfooter cell\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 2);
            assert_eq!(grid.cells.len(), 2);
            // First cell now carries a typed Shortcode::Buttons block.
            let has_typed_buttons = grid.cells[0].iter().any(|b| {
                matches!(
                    b,
                    Block::Shortcode(Shortcode::Buttons(args)) if args.items.len() == 1
                        && args.items[0].text == "Tickets"
                )
            });
            assert!(
                has_typed_buttons,
                "expected typed Buttons in cell[0]; got {:?}",
                grid.cells[0]
            );
            // Second cell is the footer paragraph.
            let has_footer_para = grid.cells[1].iter().any(|b| {
                matches!(
                    b,
                    Block::Paragraph(inlines) if inlines.iter().any(|i| matches!(
                        i,
                        super::super::node::Inline::Text(t) if t.contains("footer cell")
                    ))
                )
            });
            assert!(
                has_footer_para,
                "expected footer paragraph in cell[1]; got {:?}",
                grid.cells[1]
            );
        }
        other => panic!("expected Grid, got {other:?}"),
    }
    // The literal ::: markers don't survive verbatim — they're in the
    // typed Grid's body now.
    assert!(!result.markdown_with_placeholders.contains(":::grid 2"));
}

#[test]
fn arity_mismatch_does_not_close_block() {
    // A `:::` closer inside a `::::buttons` block must NOT close it.
    // Body content can contain `:::` strings as text (in code blocks
    // or grid cell separators when nested differently).
    let md = "::::buttons\n[t](u)\n:::\n[t2](u2)\n::::\n";
    let result = extract_shortcodes(md);
    // Only one extraction (the ::::buttons block).
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            // Both links should be captured (the `:::` was just body text).
            assert_eq!(args.items.len(), 2);
        }
        _ => panic!("expected Buttons"),
    }
}

// ---- Buttons (Phase B Task 8) ----

#[test]
fn extracts_buttons_block_with_one_link() {
    let md = ":::buttons\n[Documentation](docs/)\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert!(args.classes.is_empty());
            assert_eq!(args.items.len(), 1);
            assert_eq!(args.items[0].text, "Documentation");
            match &args.items[0].url {
                Url::Unresolved(s) => assert_eq!(s, "docs/"),
                _ => panic!("expected Unresolved"),
            }
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extracts_buttons_block_with_multiple_links() {
    let md = ":::buttons\n[Docs](docs/)\n[GitHub](https://github.com)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 2);
            assert_eq!(args.items[0].text, "Docs");
            assert_eq!(args.items[1].text, "GitHub");
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extracts_buttons_block_with_class_attrs() {
    let md = ":::buttons {.primary .large}\n[Go](go/)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.classes, "primary large");
            assert_eq!(args.items.len(), 1);
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extracts_buttons_with_moss_resolved_url_intact() {
    // The upstream resolve pipeline rewrites internal links to
    // moss-resolved:foo.md before the AST sees them. The extractor
    // must preserve the prefix verbatim — visit_urls_mut classifies it.
    let md = ":::buttons\n[Docs](moss-resolved:docs/index.md)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => match &args.items[0].url {
            Url::Unresolved(s) => assert_eq!(s, "moss-resolved:docs/index.md"),
            _ => panic!("expected Unresolved"),
        },
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn buttons_skips_non_link_lines() {
    // Non-link lines (commentary, blank lines) are silently skipped
    // — matches the legacy rewriter behavior.
    let md = ":::buttons\nNot a link, just text.\n[Real](real/)\n\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 1);
            assert_eq!(args.items[0].text, "Real");
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn buttons_inside_code_fence_is_not_extracted() {
    let md = "```\n:::buttons\n[t](u)\n:::\n```\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty());
}

#[test]
fn extract_markdown_link_rejects_text_with_close_bracket() {
    // Pinning test (code-review P2): the parser uses find(']') for the
    // first close-bracket. A link text containing ']' silently fails
    // to parse and the line is skipped (silently — matches legacy
    // shortcode.rs::extract_markdown_link). If/when this is relaxed,
    // this test fails and the change is deliberate.
    let md = ":::buttons\n[a]b](u)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => assert!(args.items.is_empty()),
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extract_markdown_link_requires_trailing_paren() {
    // Pinning test: trailing content after `)` causes the link to be
    // rejected (matches legacy behavior).
    let md = ":::buttons\n[t](u) <!-- trailing -->\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => assert!(args.items.is_empty()),
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn close_fence_with_trailing_whitespace_is_recognized() {
    // Whitespace after the colons is allowed (matches legacy
    // parse_fence_close at shortcode.rs:857).
    let md = ":::subscribe\nbutton: x\n:::   \n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
}

#[test]
fn is_close_fence_handles_multibyte_utf8_lines() {
    // Regression test for the moss-releases panic at byte index 3
    // (inside `申`) of `[申请测试版](#青苔正在封闭测试)`. The buggy
    // `split_at(arity)` was byte-indexed; this line happens to be
    // longer than 3 bytes but the first 3 bytes land mid-character
    // because `[` (1 byte) + `申` (3 bytes, bytes 1..4). Char-based
    // iteration sidesteps the issue.
    assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 3));
    assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 4));
    // CJK lines that would have panicked the old split_at variant.
    assert!(!is_close_fence("中文内容", 3));
    assert!(!is_close_fence("日本語", 3));
    // Truly closing lines still match.
    assert!(is_close_fence(":::", 3));
    assert!(is_close_fence("::::", 4));
}

#[test]
fn extract_shortcodes_handles_buttons_with_cjk_link_text() {
    // End-to-end regression for the moss-releases site bug: a
    // :::buttons block containing a markdown link with CJK text
    // and a CJK URL anchor. The extractor must not panic, must
    // extract the buttons block, and must capture both items.
    let md = ":::buttons\n[申请测试版](#青苔正在封闭测试)\n[文档](docs/)\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 2);
            assert_eq!(args.items[0].text, "申请测试版");
            match &args.items[0].url {
                Url::Unresolved(s) => assert_eq!(s, "#青苔正在封闭测试"),
                _ => panic!("expected Unresolved"),
            }
            assert_eq!(args.items[1].text, "文档");
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extract_shortcodes_does_not_panic_on_arbitrary_cjk_content() {
    // Smoke test against the shape that triggered the moss-releases
    // panic: a document with mixed CJK content INCLUDING lines that
    // start with multi-byte characters but happen to have byte
    // length ≥ arity. None of these are close-fence candidates;
    // the extractor must scan past them without panic.
    let md = "# 标题\n\n中文段落，混合 English 单词。\n\n:::buttons\n[申请测试版](#锚点)\n:::\n\n## 二级标题\n\n更多内容。\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
}

#[test]
fn close_fence_with_trailing_text_is_not_recognized() {
    // P1 #2 fix: `::: more text` does NOT close the block. Without
    // this match against legacy semantics, an author who pasted text
    // after the closer would see different behavior between the
    // typed-AST path and the legacy grid parser. Using buttons here
    // because subscribe under the unified grammar reads attrs only.
    let md = ":::buttons\n[a](u)\n::: more text\n[b](v)\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    // Both links should be in the buttons body — the first `:::` is
    // body content; the second `:::` is the closer.
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 2);
            assert_eq!(args.items[0].text, "a");
            assert_eq!(args.items[1].text, "b");
        }
        _ => panic!("expected Buttons"),
    }
}

// ---- Gallery (Phase B Task 9) ----

#[test]
fn extracts_gallery_with_bare_paths() {
    let md = ":::gallery\nphoto1.jpg\nphoto2.png\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => {
            assert!(args.columns.is_none());
            assert_eq!(args.items.len(), 2);
            assert_eq!(args.items[0].alt, "");
            match &args.items[0].src {
                Url::Unresolved(s) => assert_eq!(s, "photo1.jpg"),
                _ => panic!("expected Unresolved"),
            }
        }
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn extracts_gallery_with_columns_arg() {
    let md = ":::gallery 4\na.jpg\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => assert_eq!(args.columns, Some(4)),
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn extracts_gallery_with_classes() {
    let md = ":::gallery 3 {.showcase}\na.jpg\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => {
            assert_eq!(args.columns, Some(3));
            assert_eq!(args.classes, "showcase");
        }
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn extracts_gallery_with_markdown_image_syntax() {
    let md = ":::gallery\n![A photo](photo.jpg)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => {
            assert_eq!(args.items[0].alt, "A photo");
            match &args.items[0].src {
                Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                _ => panic!("expected Unresolved"),
            }
        }
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn extracts_gallery_with_pipe_attrs() {
    let md = ":::gallery\nphoto.jpg|cover top\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => {
            assert_eq!(args.items[0].attrs, "cover top");
            match &args.items[0].src {
                Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                _ => panic!("expected Unresolved"),
            }
        }
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn gallery_skips_blank_lines() {
    let md = ":::gallery\n\na.jpg\n\nb.jpg\n\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => assert_eq!(args.items.len(), 2),
        _ => panic!("expected Gallery"),
    }
}

// ---- Multi-line attribute blocks (Step 1 Task B) ----
// Low-level brace_depth and gather_multi_line_attrs unit tests live
// in `attrs.rs` next to those helpers. The tests below pin the
// extractor's end-to-end behavior on multi-line attribute blocks.

#[test]
fn extracts_buttons_with_multi_line_attrs() {
    // The attribute block spans three source lines; the body starts
    // after the closing brace's line.
    let md = ":::buttons {\n  .primary\n}\n[Go](go/)\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.classes, "primary");
            assert_eq!(args.items.len(), 1);
            assert_eq!(args.items[0].text, "Go");
        }
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn extracts_gallery_with_multi_line_attrs() {
    let md = ":::gallery {\n  .showcase\n}\nphoto.jpg\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Gallery(args) => {
            assert_eq!(args.classes, "showcase");
            assert_eq!(args.items.len(), 1);
        }
        _ => panic!("expected Gallery"),
    }
}

#[test]
fn multi_line_attrs_with_quoted_brace_inside() {
    // The `}` inside a quoted value must NOT close the attr block.
    // The block legitimately closes on the third line.
    let md = ":::buttons {\n  .a\n  .b\n}\n[Go](go/)\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            // Multi-line splits both classes — same as space-separated form.
            assert_eq!(args.classes, "a b");
        }
        _ => panic!("expected Buttons"),
    }
}

// ---- Pure-CSS regions (Step 1 Task D) ----

#[test]
fn css_region_unnamed_emits_div_wrapper() {
    let md = ":::{.tagline}\nA new way to publish.\n:::\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty());
    assert!(result
        .markdown_with_placeholders
        .contains("<div class=\"tagline\">"));
    assert!(result
        .markdown_with_placeholders
        .contains("A new way to publish."));
    assert!(result.markdown_with_placeholders.contains("</div>"));
}

#[test]
fn css_region_with_id_only() {
    let md = ":::{#intro}\nIntro prose.\n:::\n";
    let result = extract_shortcodes(md);
    assert!(result
        .markdown_with_placeholders
        .contains("<div id=\"intro\">"));
}

#[test]
fn css_region_with_classes_and_id() {
    let md = ":::{.callout #important}\nWatch out.\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    assert!(out.contains("<div"));
    assert!(out.contains("class=\"callout\""));
    assert!(out.contains("id=\"important\""));
}

#[test]
fn css_region_emits_blank_lines_around_body_for_markdown_processing() {
    // Pulldown-cmark needs a blank line between the `<div>` and the
    // body to treat the body as markdown rather than raw HTML.
    let md = ":::{.foo}\n# Heading\n:::\n";
    let out = extract_shortcodes(md).markdown_with_placeholders;
    // The `<div>` line is followed by a blank line.
    assert!(out.contains(">\n\n# Heading"));
    // The closing `</div>` is preceded by a blank line.
    assert!(out.contains("# Heading\n\n</div>"));
}

#[test]
fn css_region_no_warning_emitted() {
    let md = ":::{.foo}\nbody\n:::\n";
    assert!(extract_shortcodes(md).warnings.is_empty());
}

// ---- Unknown-name fallback (Step 1 Task E) ----

#[test]
fn unknown_name_renders_fallback_wrapper() {
    let md = ":::nope {.extra}\nbody text\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    assert!(out.contains("class=\"moss-unknown-shortcode extra\""));
    assert!(out.contains(r#"data-name="nope""#));
    assert!(out.contains("body text"));
}

#[test]
fn unknown_name_emits_build_warning() {
    let md = ":::nope\n:::\n";
    let warnings = extract_shortcodes(md).warnings;
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("nope"));
}

#[test]
fn unknown_name_html_escapes_data_name() {
    // Defense: a maliciously crafted name (which the opener parser
    // wouldn't actually accept since names are [A-Za-z0-9_-]) shouldn't
    // be able to break out of the attribute. This test pins the
    // escape regardless.
    let md = ":::weird-name\nbody\n:::\n";
    let out = extract_shortcodes(md).markdown_with_placeholders;
    assert!(out.contains(r#"data-name="weird-name""#));
}

// Grid left LEGACY_PASSTHROUGH in Step 2b — it's now a typed variant.
// Coverage moved to the Grid section below (extracts_grid_*).

#[test]
fn extracts_grid_with_positional_columns() {
    // Legacy moss-releases form: `:::grid 2` (positional column count)
    // with `---` cell divider. Both the positional cols and the legacy
    // `---` divider are accepted during the migration window.
    //
    // Phase 4 PR4.5 (2026-05-28): cells are now typed Vec<Vec<Block>>.
    // Each "cell A" / "cell B" parses to a single Paragraph block.
    let md = ":::grid 2\ncell A\n---\ncell B\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 2);
            assert!(grid.ratio.is_none());
            assert_eq!(grid.cells.len(), 2);
            assert_paragraph_text(&grid.cells[0], "cell A");
            assert_paragraph_text(&grid.cells[1], "cell B");
        }
        other => panic!("expected Grid, got {other:?}"),
    }
}

/// Test helper: assert that `cell_blocks` is a single `Block::Paragraph`
/// whose inline text content (concatenated) equals `expected`.
///
/// PR4.5 cells parse via pulldown-cmark; trivial cells like `"A"` yield
/// `[Block::Paragraph(vec![Inline::Text("A".into())])]`.
fn assert_paragraph_text(cell_blocks: &[Block], expected: &str) {
    if cell_blocks.is_empty() && expected.is_empty() {
        return;
    }
    let para = match cell_blocks {
        [Block::Paragraph(inlines)] => inlines,
        other => panic!("expected single Paragraph cell with text {expected:?}, got: {other:?}"),
    };
    let mut text = String::new();
    for inline in para {
        match inline {
            super::super::node::Inline::Text(t) => text.push_str(t),
            super::super::node::Inline::Code(c) => text.push_str(c),
            _ => {}
        }
    }
    assert_eq!(text, expected, "cell text mismatch");
}

#[test]
fn extracts_grid_with_positional_ratio() {
    let md = ":::grid 2 1:2\nleft\n---\nright\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 2);
            assert_eq!(grid.ratio.as_deref(), Some("1:2"));
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_with_cols_attr_integer() {
    let md = ":::grid {cols=3}\nA\n+++\nB\n+++\nC\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 3);
            assert_eq!(grid.cells.len(), 3);
            assert_paragraph_text(&grid.cells[0], "A");
            assert_paragraph_text(&grid.cells[1], "B");
            assert_paragraph_text(&grid.cells[2], "C");
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_with_cols_attr_ratio_implies_count() {
    let md = ":::grid {cols=1:1:2}\nA\n+++\nB\n+++\nC\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 3, "ratio length implies column count");
            assert_eq!(grid.ratio.as_deref(), Some("1:1:2"));
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_accepts_plus_plus_plus_divider() {
    let md = ":::grid 2\nA\n+++\nB\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.cells.len(), 2);
            assert_paragraph_text(&grid.cells[0], "A");
            assert_paragraph_text(&grid.cells[1], "B");
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_with_classes() {
    let md = ":::grid 3 {.work-cards .featured}\nA\n---\nB\n---\nC\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 3);
            assert_eq!(grid.classes, "work-cards featured");
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_single_cell_no_separator() {
    let md = ":::grid 1\nonly cell\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.columns, 1);
            assert_eq!(grid.cells.len(), 1);
            assert_paragraph_text(&grid.cells[0], "only cell");
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_with_empty_middle_cell() {
    // Two consecutive `+++` dividers leave a middle cell empty.
    // Legacy behavior preserved this; verify the typed extractor
    // does too. PR4.5: empty cells are `Vec<Block>::new()` (the
    // parser sees no content and emits zero blocks).
    let md = ":::grid 3\nA\n+++\n+++\nC\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.cells.len(), 3);
            assert_paragraph_text(&grid.cells[0], "A");
            assert!(grid.cells[1].is_empty(), "empty cell should have no blocks");
            assert_paragraph_text(&grid.cells[2], "C");
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn nested_grid_via_arity_is_unsupported_authoring() {
    // Pinning test: `::::grid` (arity 4) wrapping `:::grid` (arity 3)
    // does NOT cleanly nest. The outer fence's body captures the
    // inner literally, but `split_grid_cells` then splits the outer's
    // body on the inner's `+++` divider — mis-attributing the inner's
    // cells to the outer. There's no separate "nested-cell-divider"
    // syntax in moss, so this nesting pattern isn't supported.
    //
    // Authors who need a "grid inside a grid" should use a CSS region
    // wrapper (`::::{.outer-grid}`) and set CSS-only column rules.
    // This test pins the actual extraction behavior so a future
    // refactor that changes it is visible.
    let md = "::::grid 1\n:::grid 2\nA\n+++\nB\n:::\n::::\n";
    let result = extract_shortcodes(md);
    // The outer ::::grid is extracted; the inner is captured as
    // literal text and the +++ inside the inner triggers the outer's
    // own cell split. The inner Grid is NOT a top-level entry.
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(outer) => {
            assert_eq!(outer.columns, 1);
            // The +++ in the inner's body split the OUTER's cells,
            // which is the documented limitation.
            assert!(
                outer.cells.len() >= 2,
                "outer's body got split by inner's +++, demonstrating the \
                     unsupported-nesting failure mode"
            );
        }
        _ => panic!("expected Grid"),
    }
}

#[test]
fn extracts_grid_with_compound_link_cell_typed_as_link_card() {
    // SoCiviC pattern: a cell whose entire body is a single markdown
    // link wrapping multiple block children. Phase 4 PR4.5
    // (2026-05-28) detects this at the cell-string level (before
    // pulldown-cmark, which can't represent `[heading](url)`) and
    // emits a typed [`Block::LinkCard { url, children }`] with the
    // inner content parsed as blocks. The second cell is a plain
    // markdown link that fits the compound shape too (single line,
    // no block children) — also typed as `Block::LinkCard`.
    let md = ":::grid 2 {.work-cards}\n[![[poster.jpg]]\n#### Title\nbody](/url)\n+++\n[Card 2](/url2)\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => {
            assert_eq!(grid.classes, "work-cards");
            assert_eq!(grid.cells.len(), 2);
            match &grid.cells[0][..] {
                [Block::LinkCard { url, children }] => {
                    match url {
                        Url::Unresolved(u) => assert_eq!(u, "/url"),
                        _ => panic!("expected Unresolved /url"),
                    }
                    // children should include a Paragraph (with image)
                    // and a Heading (#### Title) — non-empty proves
                    // the inner block-parse ran.
                    assert!(!children.is_empty(), "compound-link inner blocks empty");
                }
                other => panic!("expected single LinkCard cell, got {other:?}"),
            }
            match &grid.cells[1][..] {
                [Block::LinkCard { url, .. }] => match url {
                    Url::Unresolved(u) => assert_eq!(u, "/url2"),
                    _ => panic!("expected Unresolved /url2"),
                },
                other => panic!("expected LinkCard for cell[1], got {other:?}"),
            }
        }
        _ => panic!("expected Grid"),
    }
}

// Hero left LEGACY_PASSTHROUGH in Step 2 — it's now a typed variant.
// The replacement test (`extracts_hero_block_with_no_image`) lives in
// the Hero section above.

#[test]
fn toc_now_renders_as_unknown_shortcode() {
    // Step 2c removed `:::toc` without replacement. Sites still using
    // it fall through to the moss-unknown-shortcode wrapper with a
    // build warning. moss-releases content rewrite (Step 3) deletes
    // its 3 :::toc blocks.
    let md = ":::toc\n:::\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty(), "toc is no longer typed");
    assert_eq!(result.warnings.len(), 1, "unknown-name fallback warning");
    assert!(result.warnings[0].contains("toc"));
    assert!(result
        .markdown_with_placeholders
        .contains(r#"data-name="toc""#));
}

// ---- Hero (Step 2) ----

#[test]
fn extracts_hero_block_with_no_image() {
    let md = ":::hero\n# A House of Daowu\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1, "hero should be extracted");
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert!(args.image.is_none());
            assert_eq!(args.overlay_text, "# A House of Daowu");
        }
        other => panic!("expected Hero, got {other:?}"),
    }
    // The literal `:::hero` should not survive in the output.
    assert!(!result.markdown_with_placeholders.contains(":::hero"));
}

#[test]
fn extracts_hero_block_with_wikilink_body_image() {
    let md = ":::hero\n![[panorama.jpg]]\n# Welcome\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "panorama.jpg"),
                other => panic!("expected Unresolved url, got {other:?}"),
            }
            assert_eq!(args.overlay_text, "# Welcome");
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn extracts_hero_block_with_image_attr() {
    let md = ":::hero {image=cover.jpg}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "cover.jpg"),
                other => panic!("expected Unresolved, got {other:?}"),
            }
            assert_eq!(args.overlay_text, "# Title");
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn extracts_hero_block_with_image_attr_and_pipe_attrs() {
    // The pipe character isn't in the bareword set, so values containing
    // `|` must be quoted under the unified grammar.
    let md = r#":::hero {image="cover.jpg|contain top"}
:::
"#;
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "cover.jpg"),
                _ => panic!("expected Unresolved"),
            }
            assert_eq!(args.attrs, "contain top");
        }
        _ => panic!("expected Hero"),
    }
}

#[test]
fn extracts_hero_block_with_classes() {
    let md = ":::hero {.full .center}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert_eq!(args.classes, "full center");
        }
        _ => panic!("expected Hero"),
    }
}

#[test]
fn extracts_hero_block_with_directive_line_path() {
    // Legacy syntax used by Yi-website and chps-site:
    // `:::hero ./path.jpg` (image path on the directive line, empty body).
    // Step 3 rewrites these blocks to `:::hero {image=./path.jpg}`,
    // but the typed extractor must keep producing the same Hero AST
    // node until then to avoid silently dropping the homepage hero.
    let md = ":::hero ./assets/header.png\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => match &args.image {
            Some(Url::Unresolved(s)) => assert_eq!(s, "./assets/header.png"),
            other => panic!("expected Unresolved ./assets/header.png, got {other:?}"),
        },
        _ => panic!("expected Hero"),
    }
}

#[test]
fn extracts_hero_block_with_directive_line_path_and_pipe_attrs() {
    let md = ":::hero ./bg.jpg|contain top\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "./bg.jpg"),
                _ => panic!("expected Unresolved"),
            }
            assert_eq!(args.attrs, "contain top");
        }
        _ => panic!("expected Hero"),
    }
}

#[test]
fn extracts_hero_block_with_directive_line_path_and_classes() {
    // `:::hero ./path.jpg {.landing}` — directive-line path AND
    // an attribute block (classes only, no `image=` to avoid conflict).
    let md = ":::hero ./bg.jpg {.landing}\n# Welcome\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "./bg.jpg"),
                _ => panic!("expected Unresolved"),
            }
            assert_eq!(args.classes, "landing");
            assert_eq!(args.overlay_text, "# Welcome");
        }
        _ => panic!("expected Hero"),
    }
}

// ---- Adversarial cases for Step 1 (D/E semantics) ----

#[test]
fn nested_css_region_outer_closes_at_first_inner_close() {
    // Pinning test: same-arity nested `:::{.outer}` containing
    // `:::{.inner}` is NOT a Step 1 feature. The outer block closes at
    // the inner block's `:::` because both fences are arity 3.
    // Authors who need nesting must use mismatched arities
    // (`::::{.outer}` containing `:::{.inner}`).
    //
    // This test pins the current behavior so a future regression
    // surfaces.
    let md = ":::{.outer}\n:::{.inner}\nbody\n:::\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    // The outer `<div class="outer">` opens.
    assert!(out.contains("<div class=\"outer\""));
    // The inner `:::{.inner}` opener is left as literal text in the
    // outer body — the outer fence closed at the first arity-3 `:::`.
    assert!(out.contains(":::{.inner}"));
}

#[test]
fn nested_css_region_higher_arity_outer_recurses_into_inner() {
    // `::::{.outer}` (arity 4) survives past the inner `:::{.inner}`
    // close. The extractor recurses into the outer's body, so the
    // inner CssRegion gets its own `<div class="inner">` wrapper.
    // Both wrappers are present in the rendered output.
    let md = "::::{.outer}\n:::{.inner}\nbody\n:::\n::::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    assert!(out.contains("<div class=\"outer\""));
    assert!(out.contains("<div class=\"inner\""));
    // No literal `:::{.inner}` should leak into the body.
    assert!(!out.contains(":::{.inner}"));
}

#[test]
fn css_region_containing_typed_subscribe_is_not_recursively_extracted() {
    // Same-arity nesting: outer `:::{.wrapper}` closes at the first
    // matching `:::`, so the inner `:::subscribe` is never seen.
    let md = ":::{.wrapper}\n:::subscribe\n:::\n:::\n";
    let result = extract_shortcodes(md);
    // The wrapper opens. No subscribe is extracted because the
    // outer block consumed its arity-3 closer at the inner block's
    // first `:::`.
    assert!(result
        .markdown_with_placeholders
        .contains("<div class=\"wrapper\""));
    // Subscribe is NOT extracted in Step 1.
    assert!(result.extracted.is_empty());
}

#[test]
fn higher_arity_wrapper_recursively_extracts_typed_subscribe() {
    // `::::{.wrapper}` (arity 4) keeps the inner `:::subscribe`
    // intact in its body, and the extractor recurses into the body
    // so subscribe is parsed into a typed Shortcode and replaced
    // with a sentinel. Body markdown contains the sentinel, not the
    // literal source.
    let md = "::::{.wrapper}\n:::subscribe\n:::\n::::\n";
    let result = extract_shortcodes(md);
    assert!(result
        .markdown_with_placeholders
        .contains("<div class=\"wrapper\""));
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Subscribe(_) => {}
        _ => panic!("expected Subscribe"),
    }
    // Source `:::subscribe` is replaced by a sentinel — must not
    // leak into the rendered body.
    assert!(!result.markdown_with_placeholders.contains(":::subscribe"));
}

#[test]
fn lower_arity_outer_wraps_higher_arity_typed_inner() {
    // SoCiviC pattern: `:::{.support-band}` (arity 3) wraps
    // `::::buttons` (arity 4). The outer arity-3 closer at the end
    // closes the outer, so the inner arity-4 buttons block lives
    // intact inside the outer's body. Recursive extraction picks
    // it up and emits a sentinel.
    let md = ":::{.support-band}\n## Title\n\n::::buttons {.inverted}\n[Support Us](/support)\n::::\n*footnote*\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    // Outer CssRegion wrapper.
    assert!(out.contains("<div class=\"support-band\""));
    // Inner buttons extracted as typed Shortcode.
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(args) => {
            assert_eq!(args.items.len(), 1);
        }
        _ => panic!("expected Buttons"),
    }
    // No literal `::::buttons` text should leak through.
    assert!(!out.contains("::::buttons"));
    assert!(!out.contains("::::"));
}

#[test]
fn lower_arity_outer_wraps_grid_with_buttons_in_cell() {
    // SoCiviC index pattern: 3-colon `:::{.hero-split}` outer,
    // 4-colon `::::grid 2 {.no-cards}` middle, 5-colon
    // `:::::buttons {.inverted}` innermost. The middle grid block
    // is the recursive-extraction target — its body in turn
    // contains the buttons block, but buttons-inside-grid-cells is
    // resolved by the grid renderer, not the extractor.
    let md = "::: {.hero-split}\n::::grid 2 {.no-cards}\nleft\n+++\nright\n::::\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    // Outer hero-split CssRegion wrapper.
    assert!(out.contains("<div class=\"hero-split\""));
    // Inner grid extracted as typed Grid.
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(_) => {}
        _ => panic!("expected Grid"),
    }
    // No literal `::::grid` text should leak through.
    assert!(!out.contains("::::grid"));
}

#[test]
fn unknown_name_body_recursively_extracts_typed_inner() {
    // Unknown-name fallback (e.g. typo'd `:::buttosn`) wraps in a
    // moss-unknown-shortcode div. If the body contains a higher-
    // arity typed block (e.g. nested `::::buttons`), recursion
    // picks it up so authors can debug their typo without losing
    // valid inner content.
    let md = ":::buttosn\n::::buttons\n[a](u)\n::::\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    // Unknown wrapper.
    assert!(out.contains("data-name=\"buttosn\""));
    // Inner buttons extracted.
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Buttons(_) => {}
        _ => panic!("expected Buttons"),
    }
}

#[test]
fn unknown_name_with_plus_plus_plus_in_body_passes_through() {
    // The `+++` cell divider is a Buttons-and-Grid concern, not
    // generic shortcode body syntax. Unknown blocks should emit
    // their body verbatim including any `+++` lines. Authors who
    // misspell `:::buttons` as `:::buttosn` shouldn't see their
    // dividers eaten.
    let md = ":::buttosn\n[a](u)\n+++\n[b](v)\n:::\n";
    let result = extract_shortcodes(md);
    let out = &result.markdown_with_placeholders;
    assert!(out.contains(r#"data-name="buttosn""#));
    assert!(out.contains("[a](u)"));
    assert!(out.contains("+++"));
    assert!(out.contains("[b](v)"));
}

#[test]
fn parse_shortcode_opener_recognizes_empty_name_with_attrs() {
    assert_eq!(
        parse_shortcode_opener(":::{.tagline}"),
        Some((3, "", "{.tagline}"))
    );
}

#[test]
fn parse_shortcode_opener_rejects_just_colons() {
    assert!(parse_shortcode_opener(":::").is_none());
    assert!(parse_shortcode_opener(":::   ").is_none());
}

#[test]
fn unclosed_multi_line_attrs_block_emits_verbatim() {
    // A `{` that never closes within the doc should bubble up as
    // an unclosed block (verbatim emission).
    let md = ":::buttons {\n  .primary\n[Go](go/)\n:::\n";
    let result = extract_shortcodes(md);
    // The attribute parser surfaces an UnclosedBrace error inside
    // split_positional_and_classes' brace search. The block silently
    // falls through to the unrecognized-name path → verbatim.
    // (Step 1 Task E will tighten this into an explicit warning.)
    assert!(
        result.extracted.is_empty()
            || matches!(result.extracted[0].shortcode, Shortcode::Buttons(_))
    );
    // The opener is preserved either way.
}

// ---- Deprecation warnings (Step 3 E2) ----

#[test]
fn grid_legacy_dash_emits_deprecation_warning() {
    let md = ":::grid 2\ncell A\n---\ncell B\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("deprecated"));
    assert!(result.warnings[0].contains("+++"));
}

#[test]
fn grid_plus_plus_plus_no_deprecation_warning() {
    let md = ":::grid 2\ncell A\n+++\ncell B\n:::\n";
    let result = extract_shortcodes(md);
    assert!(result.warnings.is_empty());
}

#[test]
fn hero_body_media_lines_emit_no_deprecation_warning() {
    // Body media lines are the canonical multi-slide grammar since the
    // multi-image hero — the old Priority-3 deprecation is retired.
    let md = ":::hero\n![[bg.jpg]]\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.warnings.len(), 0, "{:?}", result.warnings);
}

#[test]
fn hero_explicit_image_attr_no_deprecation_warning() {
    let md = ":::hero {image=photo.jpg}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    assert!(result.warnings.is_empty());
}

// ── spec § P9 width-flag extraction ─────────────────────────────
//
// `:::hero {full}` / `:::gallery {wide}` / `:::grid {page}` set the
// `width` field on the typed shortcode. `full` aliases to `screen`.
// Absence of a width flag leaves `width = None`, which the emitter
// turns into "no `data-width` attribute on the wrapper".

fn first_extracted(md: &str) -> Shortcode {
    let result = extract_shortcodes(md);
    result
        .extracted
        .into_iter()
        .next()
        .expect("at least one shortcode")
        .shortcode
}

#[test]
fn hero_with_full_flag_sets_width_screen() {
    let md = ":::hero {image=photo.jpg full}\n# Title\n:::\n";
    match first_extracted(md) {
        Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("screen")),
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_with_screen_flag_sets_width_screen() {
    let md = ":::hero {image=photo.jpg screen}\n# Title\n:::\n";
    match first_extracted(md) {
        Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("screen")),
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_with_wide_flag_sets_width_wide() {
    let md = ":::hero {image=photo.jpg wide}\n# Title\n:::\n";
    match first_extracted(md) {
        Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("wide")),
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_without_width_flag_leaves_width_none() {
    let md = ":::hero {image=photo.jpg}\n# Title\n:::\n";
    match first_extracted(md) {
        Shortcode::Hero(h) => assert!(h.width.is_none(), "got {:?}", h.width),
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_consecutive_leading_media_lines_become_slides() {
    // Multi-image hero: first media line = primary, the rest =
    // extra_images; blank lines between media lines don't end the run;
    // the first non-media line starts the overlay.
    let md = ":::hero\n![[a.jpg]]\n\n![[b.jpg]]\n![](c.png)\n# Michael\nDates line\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert_eq!(args.image, Some(Url::unresolved("a.jpg".to_string())));
            assert_eq!(
                args.extra_images,
                vec![
                    Url::unresolved("b.jpg".to_string()),
                    Url::unresolved("c.png".to_string())
                ]
            );
            assert!(
                args.overlay_text.contains("# Michael"),
                "{}",
                args.overlay_text
            );
            assert!(
                !args.overlay_text.contains("b.jpg"),
                "{}",
                args.overlay_text
            );
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_prose_ending_in_media_extension_stays_overlay() {
    // "Photo: alpine-meadow.jpg" is prose, not a slide — a bare path
    // with whitespace on a continuation line ends the media run.
    let md = ":::hero\n![[a.jpg]]\nPhoto: alpine-meadow.jpg\nMore text\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert!(args.extra_images.is_empty(), "{:?}", args.extra_images);
            assert!(
                args.overlay_text.contains("Photo: alpine-meadow.jpg"),
                "{}",
                args.overlay_text
            );
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_media_after_overlay_text_stays_overlay_content() {
    // A media line AFTER prose is overlay content, exactly as before —
    // only the leading run becomes slides.
    let md = ":::hero\n![[a.jpg]]\n# Title\n![[inline.jpg]]\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert!(args.extra_images.is_empty(), "{:?}", args.extra_images);
            assert!(args.overlay_text.contains("inline.jpg"));
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_image_attribute_never_collects_slides() {
    let md = ":::hero {image=hero.jpg}\n![[b.jpg]]\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert!(args.extra_images.is_empty());
            // The body media line stays overlay content in attr mode.
            assert!(args.overlay_text.contains("b.jpg"));
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_mobile_overlay_attr_is_parsed() {
    let md = ":::hero {image=hero.jpg mobile=overlay}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert_eq!(args.mobile.as_deref(), Some("overlay"));
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_without_mobile_attr_has_none() {
    let md = ":::hero {image=hero.jpg}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert!(args.mobile.is_none());
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_mobile_overlay_with_body_image_fallback() {
    let md = ":::hero {mobile=overlay}\n![[bg.jpg]]\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    match &result.extracted[0].shortcode {
        Shortcode::Hero(args) => {
            assert_eq!(args.mobile.as_deref(), Some("overlay"));
            assert!(args.image.is_some());
        }
        other => panic!("expected Hero, got {other:?}"),
    }
}

#[test]
fn hero_unknown_mobile_value_emits_warning() {
    let md = ":::hero {image=hero.jpg mobile=fullscreen}\n# Title\n:::\n";
    let result = extract_shortcodes(md);
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("unrecognized") && w.contains("fullscreen")),
        "expected warning for unknown mobile value, got: {:?}",
        result.warnings,
    );
    // The shortcode is still extracted (not dropped).
    assert_eq!(result.extracted.len(), 1);
}

#[test]
fn placeholder_preserves_block_line_count_for_source_line_accuracy() {
    // A multi-line shortcode must collapse to a placeholder occupying the
    // SAME number of lines, so the post-extraction LineLookup stays line-
    // accurate. Without padding, data-source-line drifts after the block →
    // broken editor↔preview scroll sync (the home-page grid bug).
    let md =
        "# Title\n\n:::grid 3\n[\n![](a.jpg)\n](/x)\n+++\n[\n![](b.jpg)\n](/y)\n:::\n\n## After\n";
    let input_lines = md.lines().count();
    let result = extract_shortcodes(md);
    assert_eq!(
        result.markdown_with_placeholders.lines().count(),
        input_lines,
        "placeholder must preserve the block's line count; got:\n{}",
        result.markdown_with_placeholders
    );
    // The heading after the grid must still be on its original line 13.
    let after_line = result
        .markdown_with_placeholders
        .lines()
        .position(|l| l.contains("## After"))
        .map(|p| p + 1);
    assert_eq!(after_line, Some(13), "## After should stay on line 13");
}

#[test]
fn gallery_with_page_flag_sets_width_page() {
    let md = ":::gallery 3 {page}\nphoto.jpg\n:::\n";
    match first_extracted(md) {
        Shortcode::Gallery(g) => assert_eq!(g.width.as_deref(), Some("page")),
        other => panic!("expected Gallery, got {other:?}"),
    }
}

#[test]
fn gallery_without_width_flag_leaves_width_none() {
    let md = ":::gallery 3\nphoto.jpg\n:::\n";
    match first_extracted(md) {
        Shortcode::Gallery(g) => assert!(g.width.is_none()),
        other => panic!("expected Gallery, got {other:?}"),
    }
}

#[test]
fn grid_with_wide_flag_sets_width_wide() {
    let md = ":::grid {cols=2 wide}\ncell A\n+++\ncell B\n:::\n";
    match first_extracted(md) {
        Shortcode::Grid(g) => assert_eq!(g.width.as_deref(), Some("wide")),
        other => panic!("expected Grid, got {other:?}"),
    }
}

#[test]
fn grid_with_full_flag_normalizes_to_screen() {
    let md = ":::grid {cols=2 full}\ncell A\n+++\ncell B\n:::\n";
    match first_extracted(md) {
        Shortcode::Grid(g) => assert_eq!(g.width.as_deref(), Some("screen")),
        other => panic!("expected Grid, got {other:?}"),
    }
}

#[test]
fn grid_without_width_flag_leaves_width_none() {
    let md = ":::grid 2\ncell A\n+++\ncell B\n:::\n";
    match first_extracted(md) {
        Shortcode::Grid(g) => assert!(g.width.is_none()),
        other => panic!("expected Grid, got {other:?}"),
    }
}

// ---- Recent (Phase B / Task 4.2) ----

#[test]
fn parses_recent_with_since_and_count() {
    let (sc, warns) = parse_shortcode_block(
        "recent",
        r#"{since="2026-04-01" count="5"}"#,
        "",
        &ParseConfig::default(),
    );
    assert!(warns.is_empty());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => {
            assert_eq!(args.since.as_deref(), Some("2026-04-01"));
            assert_eq!(args.count, Some(5));
            assert!(args.last.is_none());
            assert!(args.fallback_markdown.is_empty());
        }
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn parses_recent_with_last_window() {
    let (sc, _) = parse_shortcode_block("recent", r#"{last="month"}"#, "", &ParseConfig::default());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => {
            assert_eq!(args.last.as_deref(), Some("month"));
            assert!(args.since.is_none());
            assert!(args.count.is_none());
        }
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn captures_recent_body_as_fallback_markdown() {
    let body = "No posts yet. [Follow along](/).";
    let (sc, _) = parse_shortcode_block("recent", "", body, &ParseConfig::default());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => {
            assert_eq!(args.fallback_markdown, body);
        }
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn recent_with_no_args_yields_all_none() {
    let (sc, warns) = parse_shortcode_block("recent", "", "", &ParseConfig::default());
    assert!(warns.is_empty());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => {
            assert!(args.since.is_none());
            assert!(args.last.is_none());
            assert!(args.count.is_none());
            assert!(args.fallback_markdown.is_empty());
        }
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn parses_recent_with_all_three_attrs() {
    let (sc, warns) = parse_shortcode_block(
        "recent",
        r#"{since="2026-01-01" last="month" count="3"}"#,
        "",
        &ParseConfig::default(),
    );
    assert!(warns.is_empty());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => {
            assert_eq!(args.since.as_deref(), Some("2026-01-01"));
            assert_eq!(args.last.as_deref(), Some("month"));
            assert_eq!(args.count, Some(3));
        }
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn recent_with_malformed_count_yields_none_count() {
    // Tolerant parsing: a non-numeric count value drops to None
    // rather than failing the whole block. The renderer will fall
    // back to its default (10).
    let (sc, _) = parse_shortcode_block("recent", r#"{count="lots"}"#, "", &ParseConfig::default());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => assert!(args.count.is_none()),
        other => panic!("expected Recent, got {other:?}"),
    }
}

#[test]
fn recent_body_is_trimmed() {
    // Surrounding whitespace and trailing newlines do not need to
    // travel as part of the fallback markdown.
    let (sc, _) = parse_shortcode_block(
        "recent",
        "",
        "\n  hello world  \n\n",
        &ParseConfig::default(),
    );
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Recent(args) => assert_eq!(args.fallback_markdown, "hello world"),
        other => panic!("expected Recent, got {other:?}"),
    }
}

// ---- Apply ----

#[test]
fn parses_apply_directive() {
    use super::super::shortcode::ShortcodeKind;
    use super::super::visit::has_shortcode_recursive;
    let doc = crate::ast::parse(":::apply\n:::\n");
    assert!(
        has_shortcode_recursive(&doc, ShortcodeKind::Apply),
        "expected an Apply shortcode"
    );
}

#[test]
fn apply_parse_bare_has_none_overrides() {
    let (sc, warns) = parse_shortcode_block("apply", "", "", &ParseConfig::default());
    assert!(warns.is_empty());
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Apply(args) => {
            assert!(args.placeholder.is_none());
            assert!(args.button.is_none());
        }
        other => panic!("expected Apply, got {other:?}"),
    }
}

#[test]
fn apply_parse_with_overrides() {
    let (sc, _) = parse_shortcode_block(
        "apply",
        r#"{placeholder="email" button="申请"}"#,
        "",
        &ParseConfig::default(),
    );
    match sc.expect("expected Some(Shortcode)") {
        Shortcode::Apply(args) => {
            assert_eq!(args.placeholder.as_deref(), Some("email"));
            assert_eq!(args.button.as_deref(), Some("申请"));
        }
        other => panic!("expected Apply, got {other:?}"),
    }
}

#[test]
fn extracts_recent_end_to_end_with_sentinel() {
    // Full extraction path: `:::recent` opener is recognized as
    // typed-known, gets routed through parse_shortcode_block, and the
    // literal `:::recent` is replaced by a sentinel.
    let md = ":::recent {since=\"2026-04-01\" count=\"5\"}\nNo posts yet.\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Recent(args) => {
            assert_eq!(args.since.as_deref(), Some("2026-04-01"));
            assert_eq!(args.count, Some(5));
            assert_eq!(args.fallback_markdown, "No posts yet.");
        }
        other => panic!("expected Recent, got {other:?}"),
    }
    assert!(!result.markdown_with_placeholders.contains(":::recent"));
    assert!(result
        .markdown_with_placeholders
        .contains(&placeholder_for(&result.nonce, 0)));
}

// ── Inert regions: `:::` that is not live syntax (moss#903 bug 2) ──────

/// Parse + render the way the build does, so these tests pin the OUTPUT,
/// not just the extraction bookkeeping.
fn render_markdown(md: &str) -> String {
    let doc = crate::ast::parse_with_config(md, &ParseConfig::default());
    super::super::render::render_document(&doc, &super::super::hooks::DefaultHooks::new())
}

#[test]
fn shortcode_inside_an_html_comment_is_not_extracted() {
    // moss#903 bug 2, verbatim from the report: a frontlinefellowship page
    // parked a gallery inside a TODO comment. The extractor knew about code
    // fences and nothing else, so it extracted the `:::gallery`, replaced
    // lines 2-4 of the comment with a sentinel, and left the comment's own
    // `-->` stranded — pulldown-cmark then read the following prose as part
    // of the unterminated HTML block and the rest of the page vanished.
    let md = "\
# Owner page

<!-- TODO owner assets:
     :::gallery 8
     some-image.jpg
     ::: -->

Real content after the comment.
";
    let result = extract_shortcodes(md);

    // Nothing extracted, and the source round-trips byte-for-byte: the
    // comment still carries its own opener and closer.
    assert!(
        result.extracted.is_empty(),
        "commented-out shortcode must not be extracted, got {:?}",
        result.extracted
    );
    assert_eq!(
        result.markdown_with_placeholders, md,
        "an all-inert `:::` block must round-trip the source unchanged"
    );

    let html = render_markdown(md);

    // (a) no gallery is emitted.
    assert!(
        !html.contains("moss-gallery"),
        "a commented-out `:::gallery` must not render a gallery:\n{html}"
    );
    // (b) the comment structure survives intact — opener, body and closer.
    assert!(
        html.contains("<!-- TODO owner assets:"),
        "comment opener must survive:\n{html}"
    );
    assert!(
        html.contains("::: -->"),
        "comment closer must survive — this is the byte the sentinel \
         splice destroyed:\n{html}"
    );
    // (c) the production symptom: content after the comment still renders.
    assert!(
        html.contains("Real content after the comment."),
        "content after the comment must still render:\n{html}"
    );
    assert!(
        html.contains("<h1"),
        "content before the comment must still render:\n{html}"
    );
}

#[test]
fn commented_out_shortcode_does_not_corrupt_the_comment_it_lives_in() {
    // The same page, written with the closer on its own line — the variant
    // that actually corrupted output before this fix (with `::: -->` on one
    // line the extractor found no closer and bailed to verbatim; with the
    // closer alone it extracted, and the damage was visible in the page).
    //
    // Measured pre-fix output for this input:
    //
    //     <!-- TODO owner assets:
    //     <!--MOSS_SC_d12e59ff_0-->
    //     <p>--&gt;</p>
    //
    // Three separate failures in three lines: the sentinel's own `-->`
    // closed the author's comment early, so (1) the gallery sentinel was
    // swallowed as comment text and `substitute_shortcode_placeholders`
    // never saw it — the whole block silently disappeared; (2) the author's
    // real `-->` leaked into the page as visible text; (3) everything the
    // now-mispaired comment covers goes with it.
    let md = "\
# Owner page

<!-- TODO owner assets:
     :::gallery 8
     some-image.jpg
     :::
-->

Real content after the comment.
";
    let result = extract_shortcodes(md);
    assert!(
        result.extracted.is_empty(),
        "commented-out shortcode must not be extracted, got {:?}",
        result.extracted
    );
    assert_eq!(
        result.markdown_with_placeholders, md,
        "no sentinel may be spliced into an authored comment"
    );

    let html = render_markdown(md);
    assert!(!html.contains("moss-gallery"), "(a) no gallery:\n{html}");
    // (b) the comment round-trips whole, opener through closer.
    assert!(
        html.contains(
            "<!-- TODO owner assets:\n     :::gallery 8\n     some-image.jpg\n     :::\n-->"
        ),
        "(b) the comment must round-trip byte-for-byte:\n{html}"
    );
    assert!(
        !html.contains("MOSS_SC"),
        "(b) no extraction sentinel may leak into the page:\n{html}"
    );
    assert!(
        !html.contains("--&gt;"),
        "(b) the author's `-->` must stay part of the comment, not become \
         escaped body text:\n{html}"
    );
    // (c) the production symptom: content after the comment still renders.
    assert!(
        html.contains("<p>Real content after the comment.</p>"),
        "(c) content after the comment must still render:\n{html}"
    );
}

#[test]
fn shortcode_outside_a_comment_still_extracts_normally() {
    // The other half of the fix: inertness must be scoped to the comment.
    // A live `:::grid` on the same page as a commented-out one still works.
    let md = "\
<!-- TODO: :::gallery 4 -->

:::grid 2
A
+++
B
:::

after
";
    let result = extract_shortcodes(md);
    assert_eq!(
        result.extracted.len(),
        1,
        "exactly the live grid extracts, got {:?}",
        result.extracted
    );
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => assert_eq!(grid.cells.len(), 2),
        other => panic!("expected Grid, got {other:?}"),
    }
    let html = render_markdown(md);
    assert!(html.contains("moss-grid"), "live grid must render:\n{html}");
    assert!(
        !html.contains("moss-gallery"),
        "commented gallery must not render:\n{html}"
    );
    assert!(
        html.contains("after"),
        "trailing content must render:\n{html}"
    );
}

#[test]
fn shortcode_in_an_indented_code_block_is_not_extracted() {
    let md = "How to write a grid:\n\n    :::grid 2\n    A\n    :::\n\nafter\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty(), "indented code is not syntax");
    assert_eq!(result.markdown_with_placeholders, md);
}

#[test]
fn shortcode_in_an_inline_code_span_is_not_extracted() {
    let md = "Type `:::grid 2` to open a grid, then `:::` to close it.\n";
    let result = extract_shortcodes(md);
    assert!(result.extracted.is_empty(), "inline code is not syntax");
    assert_eq!(result.markdown_with_placeholders, md);
}

#[test]
fn indented_shortcode_under_a_list_item_still_extracts() {
    // The false positive that would have been worse than the bug: list-item
    // content is indented, and indenting a shortcode under a bullet must
    // not silently delete it.
    let md = "- intro\n\n    :::buttons\n    [a](/b/)\n    :::\n";
    let result = extract_shortcodes(md);
    assert_eq!(
        result.extracted.len(),
        1,
        "a shortcode indented under a list item is live, got {:?}",
        result.extracted
    );
}

#[test]
fn commented_close_fence_does_not_end_a_live_shortcode() {
    // The closer search consults the same inert map: a `:::` parked in a
    // comment inside the body used to close the block early, stranding the
    // rest of the shortcode as prose.
    let md = ":::grid 2\nA\n<!--\n:::\n-->\n+++\nB\n:::\n";
    let result = extract_shortcodes(md);
    assert_eq!(result.extracted.len(), 1);
    match &result.extracted[0].shortcode {
        Shortcode::Grid(grid) => assert_eq!(
            grid.cells.len(),
            2,
            "both cells belong to the grid; the commented `:::` is not a closer"
        ),
        other => panic!("expected Grid, got {other:?}"),
    }
}

#[test]
fn everything_after_an_unterminated_html_comment_is_inert() {
    // The one input shape this series changes behavior for. Per CommonMark an
    // unclosed `<!--` runs to end-of-input, so the page tail is comment text —
    // where before, an extraction sentinel's own `-->` could accidentally
    // rescue it. Correct, but silent, so the host warns: see
    // `pipeline::unterminated_comment_warning`.
    let md = "intro\n\n<!-- TODO owner assets\n\n:::grid 2\nA\n:::\n";
    let result = extract_shortcodes(md);
    assert!(
        result.extracted.is_empty(),
        "everything after the unclosed comment is comment text"
    );
    assert_eq!(result.markdown_with_placeholders, md);
}
