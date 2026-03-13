//! Unified media reference resolution and display attributes.
//!
//! All media reference contexts in moss (frontmatter cover, hero, gallery,
//! inline images, wikilink embeds) call into this module. It parses pipe-
//! separated display attributes (`object-fit`, `object-position`) and
//! resolves paths via the [`ContentGraph`].
//!
//! Pure Rust, zero I/O.

use crate::content_graph::ContentGraph;

// ---------------------------------------------------------------------------
// Fit — maps to CSS `object-fit`
// ---------------------------------------------------------------------------

/// CSS `object-fit` values for media display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Cover,
    Contain,
    Fill,
    None,
    ScaleDown,
}

impl Fit {
    /// Return the CSS `object-fit` value.
    pub fn to_css_value(&self) -> &str {
        match self {
            Fit::Cover => "cover",
            Fit::Contain => "contain",
            Fit::Fill => "fill",
            Fit::None => "none",
            Fit::ScaleDown => "scale-down",
        }
    }

    /// Parse from a keyword string (case-insensitive).
    ///
    /// Accepts both CSS syntax (`"scale-down"`) and space-free forms (`"scaledown"`).
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cover" => Some(Fit::Cover),
            "contain" => Some(Fit::Contain),
            "fill" => Some(Fit::Fill),
            "none" => Some(Fit::None),
            "scale-down" | "scaledown" => Some(Fit::ScaleDown),
            _ => Option::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Position — maps to CSS `object-position`
// ---------------------------------------------------------------------------

/// CSS `object-position` values for media display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Center,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Position {
    /// Return the CSS `object-position` value.
    pub fn to_css_value(&self) -> &str {
        match self {
            Position::Center => "center",
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
            Position::TopLeft => "top left",
            Position::TopRight => "top right",
            Position::BottomLeft => "bottom left",
            Position::BottomRight => "bottom right",
        }
    }

    /// Parse from a keyword string (case-insensitive).
    ///
    /// Accepts hyphenated (`"top-left"`), concatenated (`"topleft"`), and
    /// space-separated (`"top left"`) forms.
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "center" => Some(Position::Center),
            "left" => Some(Position::Left),
            "right" => Some(Position::Right),
            "top" => Some(Position::Top),
            "bottom" => Some(Position::Bottom),
            "top-left" | "topleft" | "top left" => Some(Position::TopLeft),
            "top-right" | "topright" | "top right" => Some(Position::TopRight),
            "bottom-left" | "bottomleft" | "bottom left" => Some(Position::BottomLeft),
            "bottom-right" | "bottomright" | "bottom right" => Some(Position::BottomRight),
            _ => Option::None,
        }
    }
}

// ---------------------------------------------------------------------------
// MediaAttrs
// ---------------------------------------------------------------------------

/// Parsed display attributes for a media reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAttrs {
    pub fit: Option<Fit>,
    pub position: Option<Position>,
}

impl MediaAttrs {
    /// True when no display attributes are set.
    pub fn is_empty(&self) -> bool {
        self.fit.is_none() && self.position.is_none()
    }

    /// Build an inline CSS style string, or `None` if empty.
    ///
    /// Example output: `"object-fit:contain;object-position:left"`
    pub fn to_inline_style(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut parts = Vec::new();
        if let Some(ref fit) = self.fit {
            parts.push(format!("object-fit:{}", fit.to_css_value()));
        }
        if let Some(ref pos) = self.position {
            parts.push(format!("object-position:{}", pos.to_css_value()));
        }
        Some(parts.join(";"))
    }
}

// ---------------------------------------------------------------------------
// ResolvedMedia
// ---------------------------------------------------------------------------

/// A fully resolved media reference: path + display attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMedia {
    /// Root-relative path (no leading `/`) or external URL.
    pub path: String,
    /// Parsed display attributes.
    pub attrs: MediaAttrs,
}

// ---------------------------------------------------------------------------
// Parsing functions
// ---------------------------------------------------------------------------

/// Strip `[[` and `]]` brackets from a wikilink reference, if present.
///
/// Returns the inner text. If brackets are not present, returns the input
/// unchanged.
pub fn strip_wikilink(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
        &trimmed[2..trimmed.len() - 2]
    } else {
        trimmed
    }
}

/// Split a media reference on the first `|`, returning `(path, attrs_str)`.
///
/// If there is no `|`, `attrs_str` is an empty string.
pub fn split_pipe(raw: &str) -> (&str, &str) {
    match raw.find('|') {
        Some(pos) => (&raw[..pos], &raw[pos + 1..]),
        None => (raw, ""),
    }
}

/// Parse space-separated display-attribute keywords from the portion after `|`.
///
/// Recognized keywords map to [`Fit`] and [`Position`] variants.
/// Unknown tokens are silently ignored (callers may add diagnostic reporting).
///
/// Two-word position keywords like `"top left"` are handled: if a bare
/// directional keyword (`top`, `bottom`) is followed by another (`left`,
/// `right`), they are combined.
pub fn parse_media_attrs(raw: &str) -> MediaAttrs {
    let mut fit: Option<Fit> = None;
    let mut position: Option<Position> = None;

    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i];

        // Try combining with next token for two-word positions.
        if i + 1 < tokens.len() {
            let combined = format!("{} {}", token, tokens[i + 1]);
            if let Some(pos) = Position::from_keyword(&combined) {
                position = Some(pos);
                i += 2;
                continue;
            }
        }

        // Single-token fit.
        if let Some(f) = Fit::from_keyword(token) {
            fit = Some(f);
            i += 1;
            continue;
        }

        // Single-token position.
        if let Some(pos) = Position::from_keyword(token) {
            position = Some(pos);
            i += 1;
            continue;
        }

        // Unknown token — skip.
        i += 1;
    }

    MediaAttrs { fit, position }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Returns `true` if the path looks like an external URL or data URI.
fn is_external(path: &str) -> bool {
    path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("//")
        || path.starts_with("data:")
}

/// Full pipeline: strip wikilink → split pipe → resolve path → parse attrs.
///
/// - External URLs (`http://`, `https://`, `//`, `data:`) pass through unchanged.
/// - Root-relative paths (leading `/`) have the slash stripped.
/// - Everything else is resolved via [`ContentGraph::resolve_path`], falling
///   back to the raw path if unresolved.
pub fn resolve_media_ref(raw: &str, source_path: &str, graph: &ContentGraph) -> ResolvedMedia {
    let inner = strip_wikilink(raw);
    let (path_part, attrs_str) = split_pipe(inner);
    let path_trimmed = path_part.trim();
    let attrs = parse_media_attrs(attrs_str);

    let resolved_path = if is_external(path_trimmed) {
        // External URL — passthrough.
        path_trimmed.to_string()
    } else if let Some(stripped) = path_trimmed.strip_prefix('/') {
        // Root-relative — strip leading slash.
        stripped.to_string()
    } else {
        // Resolve via content graph, fall back to raw path.
        graph
            .resolve_path(path_trimmed, source_path)
            .unwrap_or_else(|| path_trimmed.to_string())
    };

    ResolvedMedia {
        path: resolved_path,
        attrs,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    // -- Fit ----------------------------------------------------------------

    #[test]
    fn test_fit_to_css_value() {
        assert_eq!(Fit::Cover.to_css_value(), "cover");
        assert_eq!(Fit::Contain.to_css_value(), "contain");
        assert_eq!(Fit::Fill.to_css_value(), "fill");
        assert_eq!(Fit::None.to_css_value(), "none");
        assert_eq!(Fit::ScaleDown.to_css_value(), "scale-down");
    }

    #[test]
    fn test_fit_from_keyword() {
        assert_eq!(Fit::from_keyword("cover"), Some(Fit::Cover));
        assert_eq!(Fit::from_keyword("contain"), Some(Fit::Contain));
        assert_eq!(Fit::from_keyword("fill"), Some(Fit::Fill));
        assert_eq!(Fit::from_keyword("none"), Some(Fit::None));
        assert_eq!(Fit::from_keyword("scale-down"), Some(Fit::ScaleDown));
        assert_eq!(Fit::from_keyword("scaledown"), Some(Fit::ScaleDown));
    }

    #[test]
    fn test_fit_from_keyword_case_insensitive() {
        assert_eq!(Fit::from_keyword("COVER"), Some(Fit::Cover));
        assert_eq!(Fit::from_keyword("Contain"), Some(Fit::Contain));
        assert_eq!(Fit::from_keyword("Scale-Down"), Some(Fit::ScaleDown));
        assert_eq!(Fit::from_keyword("SCALEDOWN"), Some(Fit::ScaleDown));
    }

    #[test]
    fn test_fit_from_keyword_unknown() {
        assert_eq!(Fit::from_keyword("zoom"), None);
        assert_eq!(Fit::from_keyword(""), None);
        assert_eq!(Fit::from_keyword("cover "), None); // trailing space — not trimmed
    }

    // -- Position -----------------------------------------------------------

    #[test]
    fn test_position_to_css_value() {
        assert_eq!(Position::Center.to_css_value(), "center");
        assert_eq!(Position::Left.to_css_value(), "left");
        assert_eq!(Position::Right.to_css_value(), "right");
        assert_eq!(Position::Top.to_css_value(), "top");
        assert_eq!(Position::Bottom.to_css_value(), "bottom");
        assert_eq!(Position::TopLeft.to_css_value(), "top left");
        assert_eq!(Position::TopRight.to_css_value(), "top right");
        assert_eq!(Position::BottomLeft.to_css_value(), "bottom left");
        assert_eq!(Position::BottomRight.to_css_value(), "bottom right");
    }

    #[test]
    fn test_position_from_keyword_single() {
        assert_eq!(Position::from_keyword("center"), Some(Position::Center));
        assert_eq!(Position::from_keyword("left"), Some(Position::Left));
        assert_eq!(Position::from_keyword("right"), Some(Position::Right));
        assert_eq!(Position::from_keyword("top"), Some(Position::Top));
        assert_eq!(Position::from_keyword("bottom"), Some(Position::Bottom));
    }

    #[test]
    fn test_position_from_keyword_compound() {
        // Hyphenated
        assert_eq!(Position::from_keyword("top-left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("top-right"), Some(Position::TopRight));
        assert_eq!(Position::from_keyword("bottom-left"), Some(Position::BottomLeft));
        assert_eq!(Position::from_keyword("bottom-right"), Some(Position::BottomRight));

        // Concatenated
        assert_eq!(Position::from_keyword("topleft"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("bottomright"), Some(Position::BottomRight));

        // Space-separated (used when caller pre-joins tokens)
        assert_eq!(Position::from_keyword("top left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("bottom right"), Some(Position::BottomRight));
    }

    #[test]
    fn test_position_from_keyword_case_insensitive() {
        assert_eq!(Position::from_keyword("CENTER"), Some(Position::Center));
        assert_eq!(Position::from_keyword("Top-Left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("BOTTOMRIGHT"), Some(Position::BottomRight));
    }

    #[test]
    fn test_position_from_keyword_unknown() {
        assert_eq!(Position::from_keyword("middle"), None);
        assert_eq!(Position::from_keyword(""), None);
    }

    // -- MediaAttrs ---------------------------------------------------------

    #[test]
    fn test_media_attrs_is_empty() {
        let empty = MediaAttrs {
            fit: None,
            position: None,
        };
        assert!(empty.is_empty());

        let with_fit = MediaAttrs {
            fit: Some(Fit::Cover),
            position: None,
        };
        assert!(!with_fit.is_empty());

        let with_pos = MediaAttrs {
            fit: None,
            position: Some(Position::Center),
        };
        assert!(!with_pos.is_empty());
    }

    #[test]
    fn test_to_inline_style_empty() {
        let attrs = MediaAttrs {
            fit: None,
            position: None,
        };
        assert_eq!(attrs.to_inline_style(), None);
    }

    #[test]
    fn test_to_inline_style_fit_only() {
        let attrs = MediaAttrs {
            fit: Some(Fit::Contain),
            position: None,
        };
        assert_eq!(attrs.to_inline_style(), Some("object-fit:contain".into()));
    }

    #[test]
    fn test_to_inline_style_position_only() {
        let attrs = MediaAttrs {
            fit: None,
            position: Some(Position::Left),
        };
        assert_eq!(
            attrs.to_inline_style(),
            Some("object-position:left".into())
        );
    }

    #[test]
    fn test_to_inline_style_both() {
        let attrs = MediaAttrs {
            fit: Some(Fit::Cover),
            position: Some(Position::TopLeft),
        };
        assert_eq!(
            attrs.to_inline_style(),
            Some("object-fit:cover;object-position:top left".into())
        );
    }

    // -- strip_wikilink -----------------------------------------------------

    #[test]
    fn test_strip_wikilink_with_brackets() {
        assert_eq!(strip_wikilink("[[photo.jpg]]"), "photo.jpg");
        assert_eq!(strip_wikilink("[[path/to/image.png]]"), "path/to/image.png");
    }

    #[test]
    fn test_strip_wikilink_without_brackets() {
        assert_eq!(strip_wikilink("photo.jpg"), "photo.jpg");
        assert_eq!(strip_wikilink("path/to/image.png"), "path/to/image.png");
    }

    #[test]
    fn test_strip_wikilink_with_pipe() {
        assert_eq!(strip_wikilink("[[photo.jpg|cover]]"), "photo.jpg|cover");
    }

    #[test]
    fn test_strip_wikilink_with_whitespace() {
        assert_eq!(strip_wikilink("  [[photo.jpg]]  "), "photo.jpg");
    }

    #[test]
    fn test_strip_wikilink_partial_brackets() {
        // Only opening bracket — no stripping.
        assert_eq!(strip_wikilink("[[photo.jpg"), "[[photo.jpg");
        // Only closing bracket — no stripping.
        assert_eq!(strip_wikilink("photo.jpg]]"), "photo.jpg]]");
    }

    #[test]
    fn test_strip_wikilink_empty() {
        assert_eq!(strip_wikilink("[[]]"), "");
        assert_eq!(strip_wikilink(""), "");
    }

    // -- split_pipe ---------------------------------------------------------

    #[test]
    fn test_split_pipe_with_pipe() {
        assert_eq!(split_pipe("photo.jpg|cover"), ("photo.jpg", "cover"));
        assert_eq!(
            split_pipe("path/to/img.png|contain center"),
            ("path/to/img.png", "contain center")
        );
    }

    #[test]
    fn test_split_pipe_no_pipe() {
        assert_eq!(split_pipe("photo.jpg"), ("photo.jpg", ""));
        assert_eq!(split_pipe(""), ("", ""));
    }

    #[test]
    fn test_split_pipe_multiple_pipes() {
        // Only split on the first pipe.
        assert_eq!(split_pipe("a|b|c"), ("a", "b|c"));
    }

    #[test]
    fn test_split_pipe_pipe_at_edges() {
        assert_eq!(split_pipe("|cover"), ("", "cover"));
        assert_eq!(split_pipe("photo.jpg|"), ("photo.jpg", ""));
    }

    // -- parse_media_attrs --------------------------------------------------

    #[test]
    fn test_parse_attrs_fit_only() {
        let attrs = parse_media_attrs("cover");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, None);
    }

    #[test]
    fn test_parse_attrs_position_only() {
        let attrs = parse_media_attrs("center");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_parse_attrs_fit_and_position() {
        let attrs = parse_media_attrs("contain left");
        assert_eq!(attrs.fit, Some(Fit::Contain));
        assert_eq!(attrs.position, Some(Position::Left));
    }

    #[test]
    fn test_parse_attrs_two_word_position() {
        let attrs = parse_media_attrs("top left");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::TopLeft));

        let attrs2 = parse_media_attrs("cover bottom right");
        assert_eq!(attrs2.fit, Some(Fit::Cover));
        assert_eq!(attrs2.position, Some(Position::BottomRight));
    }

    #[test]
    fn test_parse_attrs_hyphenated_compound_position() {
        let attrs = parse_media_attrs("top-right");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::TopRight));

        let attrs2 = parse_media_attrs("fill bottom-left");
        assert_eq!(attrs2.fit, Some(Fit::Fill));
        assert_eq!(attrs2.position, Some(Position::BottomLeft));
    }

    #[test]
    fn test_parse_attrs_unknown_tokens_ignored() {
        let attrs = parse_media_attrs("cover unknown-token left");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, Some(Position::Left));
    }

    #[test]
    fn test_parse_attrs_empty_string() {
        let attrs = parse_media_attrs("");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_only_whitespace() {
        let attrs = parse_media_attrs("   ");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_all_unknown() {
        let attrs = parse_media_attrs("foo bar baz");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_case_insensitive() {
        let attrs = parse_media_attrs("COVER CENTER");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_parse_attrs_last_wins_for_duplicates() {
        // If multiple fit keywords appear, the last one wins.
        let attrs = parse_media_attrs("cover contain");
        assert_eq!(attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_parse_attrs_scale_down() {
        let attrs = parse_media_attrs("scale-down");
        assert_eq!(attrs.fit, Some(Fit::ScaleDown));
    }

    // -- resolve_media_ref --------------------------------------------------

    fn sample_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("images/photo.jpg", "images/photo");
        b.add_file("assets/banner.png", "assets/banner");
        b.add_file("posts/hello.md", "posts/hello");
        b.build()
    }

    #[test]
    fn test_resolve_simple_path() {
        let graph = sample_graph();
        let result = resolve_media_ref("photo.jpg", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_with_attrs() {
        let graph = sample_graph();
        let result = resolve_media_ref("photo.jpg|cover center", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
        assert_eq!(result.attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_resolve_wikilink() {
        let graph = sample_graph();
        let result = resolve_media_ref("[[photo.jpg|contain]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_resolve_wikilink_no_attrs() {
        let graph = sample_graph();
        let result = resolve_media_ref("[[photo.jpg]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_external_http() {
        let graph = sample_graph();
        let result = resolve_media_ref(
            "https://example.com/img.jpg|cover",
            "posts/hello.md",
            &graph,
        );
        assert_eq!(result.path, "https://example.com/img.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
    }

    #[test]
    fn test_resolve_external_protocol_relative() {
        let graph = sample_graph();
        let result = resolve_media_ref("//cdn.example.com/img.jpg", "posts/hello.md", &graph);
        assert_eq!(result.path, "//cdn.example.com/img.jpg");
    }

    #[test]
    fn test_resolve_external_data_uri() {
        let graph = sample_graph();
        let result = resolve_media_ref("data:image/png;base64,abc", "posts/hello.md", &graph);
        assert_eq!(result.path, "data:image/png;base64,abc");
    }

    #[test]
    fn test_resolve_root_relative() {
        let graph = sample_graph();
        let result = resolve_media_ref("/images/photo.jpg|fill", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Fill));
    }

    #[test]
    fn test_resolve_unresolved_fallback() {
        let graph = sample_graph();
        let result = resolve_media_ref("missing.jpg", "posts/hello.md", &graph);
        // ContentGraph returns None → fallback to raw path.
        assert_eq!(result.path, "missing.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_wikilink_with_two_word_position() {
        let graph = sample_graph();
        let result =
            resolve_media_ref("[[banner.png|cover top left]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "assets/banner.png");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
        assert_eq!(result.attrs.position, Some(Position::TopLeft));
    }

    #[test]
    fn test_resolve_external_in_wikilink() {
        let graph = sample_graph();
        let result = resolve_media_ref(
            "[[https://example.com/img.jpg|contain]]",
            "posts/hello.md",
            &graph,
        );
        assert_eq!(result.path, "https://example.com/img.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_resolve_path_with_spaces_trimmed() {
        let graph = sample_graph();
        let result = resolve_media_ref("  photo.jpg  | cover ", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
    }
}
