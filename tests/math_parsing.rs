//! P1 LaTeX math: parse + honest fallback.
//!
//! The class of bug this file exists to prevent: enabling
//! `Options::ENABLE_MATH` without handler arms makes pulldown-cmark emit
//! `Event::InlineMath` / `Event::DisplayMath`, which the typed-AST parser's
//! catch-all arms drop on the floor — `Energy $E = mc^2$.` renders as
//! `<p>Energy .</p>`. Silent deletion of the author's content. Every test
//! below is a guard against that, not a test of typesetting (there is no
//! engine in P1 — math renders as escaped source in `<code class="moss-math">`).
//!
//! See docs/plans/2026-07-21-latex-math-design.md §4 (P1) and ADR-030.

use moss_core::ast::{parse_with_config, render_document, DefaultHooks, ParseConfig};

fn render(markdown: &str, math: bool) -> String {
    let config = ParseConfig {
        math,
        ..Default::default()
    };
    let doc = parse_with_config(markdown, &config);
    render_document(&doc, &DefaultHooks::new())
}

// ---------------------------------------------------------------------------
// The regression guard — math must never be silently dropped
// ---------------------------------------------------------------------------

/// THE P1 TEST. Measured pre-fix behavior with ENABLE_MATH and no arms:
/// `<p>Energy .</p>` — the equation vanished. If this ever regresses, the
/// author's content is being deleted by the build.
#[test]
fn math_is_never_silently_dropped() {
    let html = render("Energy $E = mc^2$.", true);

    assert!(
        !html.contains("<p>Energy .</p>"),
        "math was SILENTLY DELETED (the ENABLE_MATH-without-arms bug): {html}"
    );
    assert!(
        html.contains("E = mc^2"),
        "the LaTeX source must survive to the output: {html}"
    );
    // The surrounding prose must be intact too.
    assert!(html.contains("Energy "), "prose lost: {html}");
}

// ---------------------------------------------------------------------------
// Inline math
// ---------------------------------------------------------------------------

#[test]
fn inline_math_renders_as_moss_math_code_span() {
    let html = render("$E = mc^2$", true);
    assert!(
        html.contains(r#"<code class="moss-math" data-moss-math="inline">E = mc^2</code>"#),
        "unexpected inline math markup: {html}"
    );
}

#[test]
fn inline_math_is_inert_when_math_is_off() {
    let html = render("$E = mc^2$", false);
    assert!(
        !html.contains("moss-math"),
        "math:false must not parse math: {html}"
    );
    assert!(
        html.contains("$E = mc^2$"),
        "with math off the source must pass through literally: {html}"
    );
}

/// `ParseConfig::default()` must leave math OFF, so the ~40 in-crate
/// `parse()` callers and every existing snapshot fixture are unaffected.
/// Production opts in via `[site].math` on `SiteConfig`.
#[test]
fn math_defaults_to_off() {
    assert!(!ParseConfig::default().math);
    let doc = moss_core::ast::parse("$E = mc^2$");
    let html = render_document(&doc, &DefaultHooks::new());
    assert!(
        html.contains("$E = mc^2$"),
        "default parse() changed: {html}"
    );
}

// ---------------------------------------------------------------------------
// Display math
// ---------------------------------------------------------------------------

#[test]
fn display_math_renders_with_display_marker() {
    let html = render("$$ x^2 $$", true);
    assert!(
        html.contains(r#"data-moss-math="display""#),
        "display math must be marked as display: {html}"
    );
    assert!(html.contains("x^2"), "display TeX lost: {html}");
}

#[test]
fn display_math_block_survives_on_its_own_lines() {
    let html = render("Before\n\n$$\n\\frac{a}{b}\n$$\n\nAfter", true);
    assert!(html.contains(r#"data-moss-math="display""#), "{html}");
    assert!(html.contains(r"\frac{a}{b}"), "{html}");
    assert!(html.contains("Before"), "{html}");
    assert!(html.contains("After"), "{html}");
}

// ---------------------------------------------------------------------------
// Escaping — TeX is full of characters that are HTML metacharacters
// ---------------------------------------------------------------------------

#[test]
fn math_source_is_html_escaped() {
    let html = render("$a < b & c$", true);
    assert!(
        html.contains("a &lt; b &amp; c"),
        "TeX must be HTML-escaped: {html}"
    );
    // No raw metacharacter may survive inside the math span.
    let span = html
        .split(r#"data-moss-math="inline">"#)
        .nth(1)
        .and_then(|s| s.split("</code>").next())
        .unwrap_or_else(|| panic!("no math span in {html}"));
    assert_eq!(span, "a &lt; b &amp; c");
    // No raw `<`, and every `&` must open an entity (a bare `&` would mean
    // the escaper ran on `<` but not `&`, or ran twice on one and not the
    // other).
    assert!(!span.contains('<'), "raw `<` in math span: {span:?}");
    assert!(
        span.replace("&lt;", "")
            .replace("&amp;", "")
            .find('&')
            .is_none(),
        "unescaped `&` in math span: {span:?}"
    );
}

#[test]
fn math_cannot_inject_markup() {
    let html = render(r#"$</code><script>alert(1)</script>$"#, true);
    assert!(
        !html.contains("<script>"),
        "math span allowed script injection: {html}"
    );
}

// ---------------------------------------------------------------------------
// Wiring: math inside constructs whose inlines flow through
// `parse_inline_event`'s whitelist (list items, callouts, table cells).
// A missing whitelist entry drops math ONLY here — parse_inline alone
// looks green. This is the mechanism-vs-wiring guard.
// ---------------------------------------------------------------------------

#[test]
fn math_survives_inside_list_items() {
    let html = render("- energy $E = mc^2$ here\n- plain", true);
    assert!(
        html.contains("E = mc^2"),
        "math dropped inside a list item — parse_inline_event whitelist is \
         missing the math events: {html}"
    );
}

#[test]
fn math_survives_inside_a_table_cell() {
    let html = render("| a | b |\n|---|---|\n| $x^2$ | y |", true);
    assert!(
        html.contains("x^2"),
        "math dropped inside a table cell: {html}"
    );
}
