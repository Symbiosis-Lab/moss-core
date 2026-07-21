//! The P1 math fallback node and its inverse.
//!
//! P1's contract is that math is **never silently deleted**. Two shapes
//! carry an equation through the AST, and this module owns both plus the
//! conversion between them, so the format and its parser cannot drift:
//!
//! | Shape | Built by | Used where |
//! |---|---|---|
//! | `<code class="moss-math" data-moss-math="…">` in [`Inline::Other`] | [`math_inline`] | rendered HTML |
//! | `$…$` / `$$…$$` markdown source | [`math_source`] | every plain-text collector |
//!
//! **Why plain-text collectors get the source and not bare TeX.** pulldown
//! hands a walker the *inner* TeX only — delimiters stripped, inner bytes
//! otherwise untouched — so `$` + tex + `$` reproduces the author's
//! original bytes exactly. Restoring the delimiters is what keeps three
//! independent surfaces in agreement:
//!
//! 1. **The math=off slug.** `# Euler $e^{i\pi}=-1$ identity` must produce
//!    the same `<h1 id>` whether or not `[site].math` is on, or flipping
//!    the flag silently breaks every existing deep link, in-page TOC entry
//!    and `[[Page#Heading]]` wikilink pointing at that heading.
//! 2. **The wikilink graph.** `build/scan/scan.rs` slugs headings off the
//!    RAW heading line; being a line scanner it cannot know where math
//!    begins, so it always includes the `$` bytes. The render side has to
//!    match it, or the graph resolves a link to a fragment the page lacks.
//! 3. **`extract_headings`**, whose module doc calls byte-identity with the
//!    rendered `<hN id>` "the keystone invariant".
//!
//! Bare TeX would satisfy none of the three. Markup would be actively wrong
//! in these contexts — `alt` is an HTML attribute and the slug feeds a URL
//! fragment.

use super::hooks::escape_text;
use super::node::Inline;

const PREFIX_INLINE: &str = r#"<code class="moss-math" data-moss-math="inline">"#;
const PREFIX_DISPLAY: &str = r#"<code class="moss-math" data-moss-math="display">"#;
const SUFFIX: &str = "</code>";

/// Build the P1 math fallback node: the equation's own LaTeX source,
/// HTML-escaped, in a marked `<code>` span.
///
/// The `data-moss-math` attribute carries display-vs-inline so a later
/// phase's renderer can typeset from the AST without re-deriving it, and so
/// the CSS can size display math differently without a second class.
pub(crate) fn math_inline(tex: &str, display: bool) -> Inline {
    let prefix = if display { PREFIX_DISPLAY } else { PREFIX_INLINE };
    Inline::Other(format!("{prefix}{}{SUFFIX}", escape_text(tex)))
}

/// Reconstruct an equation's markdown source — the TeX with its `$` / `$$`
/// delimiters restored — from the inner TeX pulldown hands the walker.
pub(crate) fn math_source(tex: &str, display: bool) -> String {
    let delim = if display { "$$" } else { "$" };
    format!("{delim}{tex}{delim}")
}

/// Recover the markdown source of a math node from an [`Inline::Other`]
/// payload, or `None` if the payload is some other raw-HTML passthrough.
///
/// The inverse of [`math_inline`]. Plain-text walkers that see the AST
/// rather than the event stream (`extract_headings::inlines_to_text`,
/// `extract_hero::inlines_plain_text`) have no access to the original
/// event, so recovering from the node is the only way for them to honor
/// P1's never-silently-delete contract. Round-tripping is pinned by
/// `math_source_round_trips_through_the_node` below — that test is what
/// keeps this from drifting away from the builder three lines above it.
pub(crate) fn math_source_from_other(html: &str) -> Option<String> {
    let (prefix, display) = if html.starts_with(PREFIX_DISPLAY) {
        (PREFIX_DISPLAY, true)
    } else if html.starts_with(PREFIX_INLINE) {
        (PREFIX_INLINE, false)
    } else {
        return None;
    };
    let inner = html
        .strip_prefix(prefix)?
        .strip_suffix(SUFFIX)?
        // Only the three characters `escape_text` writes, unescaped in the
        // order that makes `&amp;lt;` come back as the literal `&lt;`
        // rather than as `<`.
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    Some(math_source(&inner, display))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_restores_delimiters() {
        assert_eq!(math_source("E=mc^2", false), "$E=mc^2$");
        assert_eq!(math_source("a+b", true), "$$a+b$$");
    }

    #[test]
    fn math_source_round_trips_through_the_node() {
        // Every TeX here exercises a character `escape_text` rewrites, so a
        // change to either side of the escape pair fails this test.
        for tex in ["E=mc^2", "a < b", "a > b", "x &amp; y", "S = \\{x : x > 0\\}"] {
            for display in [false, true] {
                let Inline::Other(html) = math_inline(tex, display) else {
                    panic!("math_inline must build an Inline::Other");
                };
                assert_eq!(
                    math_source_from_other(&html).as_deref(),
                    Some(math_source(tex, display).as_str()),
                    "round-trip failed for {tex:?} display={display}"
                );
            }
        }
    }

    #[test]
    fn non_math_passthrough_is_not_claimed() {
        assert_eq!(math_source_from_other("<div>hello</div>"), None);
        assert_eq!(math_source_from_other("<code>plain</code>"), None);
        // A `moss-math-error` node (P2) must not be mistaken for a fallback.
        assert_eq!(
            math_source_from_other(r#"<code class="moss-math-error">x</code>"#),
            None
        );
    }
}
