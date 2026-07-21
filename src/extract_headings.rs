//! Pure extraction of a document's headings (text + slug + level) for the
//! editor's `[[Page#Heading]]` autocomplete. Reuses `parse()` (which runs
//! `assign_heading_id_suffixes`) so the returned slugs are byte-identical
//! to the rendered `<hN id="...">` attributes — the keystone invariant.
//!
//! v1 extracts TOP-LEVEL headings only (the common case). Headings nested
//! inside callouts / blockquotes / lists are not offered for autocomplete;
//! a recursive walk is a follow-up if needed.

use crate::ast::math_text::math_source_from_other;
use crate::ast::parser::ParseConfig;
use crate::ast::{parse_with_config, Block, Inline};

/// A heading discovered in a document, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingInfo {
    /// Plain-text heading title (inline markup flattened).
    pub text: String,
    /// Final deduped slug — matches the rendered `<hN id="...">`.
    pub slug: String,
    /// Heading level 1..=6.
    pub level: u8,
}

/// Flatten an inline slice to its plain-text content (markup removed).
fn inlines_to_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(t),
            Inline::Code(c) => out.push_str(c),
            Inline::Emphasis(children) | Inline::Strong(children) => {
                inlines_to_text(children, out)
            }
            Inline::Link { children, .. } => inlines_to_text(children, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::LineBreak => out.push(' '),
            // A math fallback node is raw HTML, but it is the only
            // `Inline::Other` that carries author text. Recover its
            // markdown source so `text` agrees with `slug` — the slug
            // keeps `$…$` (see `ast::math_text`), so the label must too,
            // or autocomplete shows a title the anchor does not match.
            Inline::Other(html) => {
                if let Some(src) = math_source_from_other(html) {
                    out.push_str(&src);
                }
            }
        }
    }
}

/// Extract all top-level headings from `markdown` in document order, with
/// final (deduped) slugs identical to the rendered `<hN id>`.
///
/// Uses [`ParseConfig::default`], which has math **off**. A site with
/// `[site].math` on must call [`extract_headings_with_config`] instead:
/// with math off, `$…$` is ordinary text and the slug happens to come out
/// right, but that is a coincidence of this release's delimiter-preserving
/// design and not something callers should lean on.
pub fn extract_headings(markdown: &str) -> Vec<HeadingInfo> {
    extract_headings_with_config(markdown, &ParseConfig::default())
}

/// [`extract_headings`], parsing with the caller's [`ParseConfig`].
///
/// The keystone invariant is byte-identity with the rendered `<hN id>`, and
/// the render path parses with the *site's* config. Parsing here with a
/// different one is therefore a way to violate the invariant without
/// touching any slug logic, which is exactly what happened while this
/// function called the bare `parse()`.
pub fn extract_headings_with_config(markdown: &str, config: &ParseConfig) -> Vec<HeadingInfo> {
    let doc = parse_with_config(markdown, config);
    let mut out = Vec::new();
    for block in &doc.blocks {
        if let Block::Heading {
            level,
            children,
            id,
        } = block
        {
            let mut text = String::new();
            inlines_to_text(children, &mut text);
            out.push(HeadingInfo {
                text: text.trim().to_string(),
                slug: id.clone().unwrap_or_default(),
                level: *level,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_slug_level() {
        let md = "# Title\n\n## Getting Started\n\ntext\n\n### Sub *em*\n";
        let hs = extract_headings(md);
        assert_eq!(hs.len(), 3);
        assert_eq!(hs[0], HeadingInfo { text: "Title".into(), slug: "title".into(), level: 1 });
        assert_eq!(hs[1], HeadingInfo { text: "Getting Started".into(), slug: "getting-started".into(), level: 2 });
        assert_eq!(hs[2], HeadingInfo { text: "Sub em".into(), slug: "sub-em".into(), level: 3 });
    }

    #[test]
    fn dedups_duplicate_slugs() {
        let md = "## Setup\n\n## Setup\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, "setup");
        assert_eq!(hs[1].slug, "setup-1");
    }

    #[test]
    fn preserves_cjk() {
        let md = "## 中文标题\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, "中文标题");
        assert_eq!(hs[0].text, "中文标题");
    }

    #[test]
    fn empty_doc_no_headings() {
        assert!(extract_headings("just a paragraph\n").is_empty());
    }

    #[test]
    fn slug_matches_obsidian_anchor_for_punctuation() {
        // Keystone: slug must equal what obsidian_heading_anchor produces
        // (the same fn the renderer uses for id=). Spot-check a heading with
        // punctuation that the algorithm keeps/strips distinctively.
        let md = "## Step 1: Install\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, crate::heading_anchor::obsidian_heading_anchor("Step 1: Install"));
    }

    /// The keystone invariant, with math in the heading — the case that
    /// broke it. The slug the walker extracts, the `<hN id>` the renderer
    /// emits, and the raw-line slug the wikilink graph computes in
    /// `build/scan/scan.rs` must agree byte-for-byte. That is the invariant
    /// that holds unconditionally, because all three see the same `$` bytes.
    ///
    /// **Agreement with the math=OFF slug is NOT universal**, and asserting
    /// it as such was wrong. With math off the TeX is ordinary markdown, so
    /// any markdown-active character inside it is consumed before slugging:
    /// `$f*g$ and $h*k$` has its two `*` eaten as emphasis, and `$V^*$ and
    /// $W^*$` — plain dual-space notation — likewise. See the `*` cases
    /// below and ADR-030 §"Upgrade-time anchor movement".
    #[test]
    fn math_heading_slug_is_identical_across_every_surface() {
        let md = "# Euler $e^{i\\pi}=-1$ identity\n";
        let math_on = ParseConfig { math: true, ..Default::default() };

        let extracted = extract_headings_with_config(md, &math_on);
        assert_eq!(extracted.len(), 1);

        // 1 ↔ 2: extracted slug == the id the renderer emits.
        let doc = crate::ast::parse_with_config(md, &math_on);
        let Block::Heading { id, .. } = &doc.blocks[0] else {
            panic!("expected a heading, got {:?}", doc.blocks[0]);
        };
        assert_eq!(extracted[0].slug, *id.as_ref().expect("heading must have an id"));

        // 3: the wikilink graph slugs the RAW heading line. THIS is the
        // strong one — a mismatch resolves a link to a fragment the page
        // does not have.
        assert_eq!(
            extracted[0].slug,
            crate::heading_anchor::obsidian_heading_anchor("Euler $e^{i\\pi}=-1$ identity")
        );

        // 4: this TeX has no markdown-active characters, so the math=OFF
        // slug happens to coincide too. Conditional, not universal.
        let off = extract_headings_with_config(md, &ParseConfig::default());
        assert_eq!(
            extracted[0].slug, off[0].slug,
            "TeX with no markdown-active characters must slug identically either way"
        );

        // And the human-readable label keeps the equation rather than
        // showing a hole where it used to be.
        assert_eq!(extracted[0].text, "Euler $e^{i\\pi}=-1$ identity");
        assert_eq!(extracted[0].text, off[0].text);
    }

    /// Pins the exception, so nobody re-asserts the false universal.
    ///
    /// `*` inside TeX is emphasis to a math-OFF parser. Turning `[site].math`
    /// on therefore MOVES these anchors — a real, user-visible upgrade cost
    /// recorded in ADR-030 and the moss-core CHANGELOG. What must still hold
    /// is graph agreement: math-ON slug == the raw-line slug the wikilink
    /// scanner computes, so links and anchors never disagree on a live site.
    #[test]
    fn markdown_active_chars_in_tex_move_the_anchor_but_keep_graph_agreement() {
        let math_on = ParseConfig { math: true, ..Default::default() };
        for (md, raw, expect_off) in [
            (
                "# Convolution $f*g$ and $h*k$ end\n",
                "Convolution $f*g$ and $h*k$ end",
                "convolution-$fg$-and-$hk$-end",
            ),
            ("# Dual $V^*$ and $W^*$ end\n", "Dual $V^*$ and $W^*$ end", "dual-$v$-and-$w$-end"),
        ] {
            let on = extract_headings_with_config(md, &math_on);
            let off = extract_headings_with_config(md, &ParseConfig::default());

            // Graph agreement — unconditional.
            assert_eq!(
                on[0].slug,
                crate::heading_anchor::obsidian_heading_anchor(raw),
                "math-ON slug diverged from the raw-line slug the wikilink graph computes"
            );

            // The documented divergence: emphasis ate the `*` with math off.
            assert_eq!(off[0].slug, expect_off, "math-OFF slug drifted from what ADR-030 records");
            assert_ne!(
                on[0].slug, off[0].slug,
                "expected this heading's anchor to MOVE when math is enabled"
            );
        }
    }

    /// `$$…$$` has no markdown-active characters here, so both slugs agree.
    #[test]
    fn display_math_in_a_heading_keeps_both_delimiters() {
        let math_on = ParseConfig { math: true, ..Default::default() };
        let hs = extract_headings_with_config("# Case $$a+b$$ tail\n", &math_on);
        assert_eq!(hs[0].text, "Case $$a+b$$ tail");
        assert_eq!(
            hs[0].slug,
            crate::heading_anchor::obsidian_heading_anchor("Case $$a+b$$ tail"),
            "graph agreement — the invariant that always holds"
        );
        assert_eq!(
            hs[0].slug,
            extract_headings_with_config("# Case $$a+b$$ tail\n", &ParseConfig::default())[0].slug
        );
    }

    /// Two headings differing only inside their math must stay distinct.
    /// Dropping the TeX collapsed them to the same base slug, so the second
    /// silently acquired a `-1` suffix and the anchors became order-dependent.
    #[test]
    fn headings_differing_only_inside_math_do_not_collide() {
        let math_on = ParseConfig { math: true, ..Default::default() };
        let hs = extract_headings_with_config("## Case $a$\n\n## Case $b$\n", &math_on);
        assert_ne!(hs[0].slug, hs[1].slug);
        assert!(!hs[1].slug.ends_with("-1"), "slug {:?} collided", hs[1].slug);
    }
}
