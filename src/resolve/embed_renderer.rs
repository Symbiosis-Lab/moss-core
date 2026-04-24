//! Renderer registry for `![[file]]` embeds.
//!
//! Each renderer maps a file extension (or extension family) to an output
//! format. The caller resolves the embed target via the ContentGraph, then
//! dispatches to the renderer for the target's extension. Unknown extensions
//! fall back to a file link (Obsidian parity) — that fallback lives in the
//! caller, not here.

/// An embed that has been parsed and path-resolved, ready for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEmbed<'a> {
    /// Resolved target path, as returned by the ContentGraph.
    pub resolved_path: &'a str,
    /// The calling file's path (for computing relative asset URLs).
    pub from_path: &'a str,
    /// `?query` from the source wikilink, without the leading `?`.
    pub query: Option<&'a str>,
    /// `#fragment` from the source wikilink, without the leading `#`.
    /// For `.md` renderers this is a heading/block-ref marker (block refs
    /// keep their `^` prefix). For every other renderer this is a URL fragment.
    pub section: Option<&'a str>,
    /// `|pipe-content` from the source wikilink. Image renderer uses this
    /// for display keywords / size; other renderers parse per their convention.
    pub alias: Option<&'a str>,
}

/// Output of a renderer.
#[derive(Debug, PartialEq, Eq)]
pub enum RenderedEmbed {
    /// Inline markdown/HTML text to splice into the output stream.
    Inline(String),
}

/// A renderer converts a `ParsedEmbed` into its rendered form.
pub trait EmbedRenderer: Send + Sync {
    /// Extensions this renderer claims (lowercase, without leading dot).
    fn extensions(&self) -> &[&'static str];

    /// Render the embed. Must be pure; moss-core is I/O-free.
    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed;
}

// ---------------------------------------------------------------------------
// ImageRenderer
// ---------------------------------------------------------------------------

use crate::heading_anchor::obsidian_heading_anchor;
use crate::media::{format_img_tag, is_all_display_keywords, parse_media_attrs};

use super::fuzzy_path::relative_asset_path;

/// Image file extensions recognized by `ImageRenderer`.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Renderer for image embeds: `![[photo.jpg]]` → `<img>` or `![alt](url)`.
pub struct ImageRenderer;

impl EmbedRenderer for ImageRenderer {
    fn extensions(&self) -> &[&'static str] {
        IMAGE_EXTENSIONS
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let out = match embed.alias {
            Some(alias_text) if is_all_display_keywords(alias_text) => {
                let alt = file_stem(embed.resolved_path);
                let attrs = parse_media_attrs(alias_text);
                format_img_tag(&url, &alt, &attrs)
            }
            Some(alias_text) => format!("![{}]({})", alias_text, url),
            None => {
                let alt = file_stem(embed.resolved_path);
                format!("![{}]({})", alt, url)
            }
        };
        RenderedEmbed::Inline(out)
    }
}

pub(crate) fn file_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.rfind('.') {
        Some(pos) if pos > 0 => filename[..pos].to_string(),
        _ => filename.to_string(),
    }
}

// ---------------------------------------------------------------------------
// MarkdownEmbedRenderer
// ---------------------------------------------------------------------------

/// Renderer for markdown transclusion: `![[file.md]]` → `<!-- moss-embed:path -->`.
///
/// The marker comment is resolved later by src-tauri's embed resolver, which
/// reads the target file's content and splices it inline. This renderer does
/// not perform I/O.
pub struct MarkdownEmbedRenderer;

impl EmbedRenderer for MarkdownEmbedRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["md"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let anchor = build_embed_anchor(embed.section);
        RenderedEmbed::Inline(format!(
            "<!-- moss-embed:{}{} -->",
            embed.resolved_path, anchor
        ))
    }
}

/// Build the anchor fragment for a markdown embed marker.
///
/// Preserves the `^` prefix on block references so the downstream embed
/// resolver can distinguish them from headings.
fn build_embed_anchor(section: Option<&str>) -> String {
    match section {
        None => String::new(),
        Some(s) if s.is_empty() => String::new(),
        Some(s) => {
            if s.starts_with('^') {
                format!("#{}", s)
            } else {
                format!("#{}", obsidian_heading_anchor(s))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyRenderer;
    impl EmbedRenderer for DummyRenderer {
        fn extensions(&self) -> &[&'static str] {
            &["xyz"]
        }
        fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
            RenderedEmbed::Inline(format!("<dummy src={}>", embed.resolved_path))
        }
    }

    #[test]
    fn test_dummy_renderer_trait_surface() {
        let r = DummyRenderer;
        assert_eq!(r.extensions(), &["xyz"]);
        let embed = ParsedEmbed {
            resolved_path: "a.xyz",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("<dummy src=a.xyz>".to_string())
        );
    }

    // --- MarkdownEmbedRenderer ---

    #[test]
    fn test_markdown_embed_renderer_no_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "posts/intro.md",
            from_path: "index.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("<!-- moss-embed:posts/intro.md -->".to_string())
        );
    }

    #[test]
    fn test_markdown_embed_renderer_heading_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "guide.md",
            from_path: "index.md",
            query: None,
            section: Some("Getting Started"),
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("<!-- moss-embed:guide.md#getting-started -->".to_string())
        );
    }

    #[test]
    fn test_markdown_embed_renderer_block_ref_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "guide.md",
            from_path: "index.md",
            query: None,
            section: Some("^block-xyz"),
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("<!-- moss-embed:guide.md#^block-xyz -->".to_string())
        );
    }

    #[test]
    fn test_markdown_embed_renderer_extensions() {
        assert_eq!(MarkdownEmbedRenderer.extensions(), &["md"]);
    }

    // --- ImageRenderer ---

    #[test]
    fn test_image_renderer_no_alias() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "assets/photo.jpg",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![photo](../assets/photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_alias_plain_text() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some("A lovely cat"),
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![A lovely cat](photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_display_keywords() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some("contain"),
        };
        let out = match r.render(&embed) {
            RenderedEmbed::Inline(s) => s,
        };
        assert!(out.starts_with("<img "), "expected <img tag, got: {}", out);
        assert!(out.contains("src=\"photo.jpg\""), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_extensions_cover_all_formats() {
        let r = ImageRenderer;
        let exts: Vec<&&str> = r.extensions().iter().collect();
        for e in &["png", "jpg", "jpeg", "gif", "svg", "webp"] {
            assert!(
                exts.iter().any(|&&x| x == *e),
                "missing ext: {} in {:?}",
                e,
                exts
            );
        }
    }
}
