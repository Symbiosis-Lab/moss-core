//! Block-level and inline-level AST nodes.
//!
//! Closed enums; pattern matching is the visitor framework. The variants
//! cover what moss emits today (CommonMark + GFM extensions enabled in the
//! pipeline: tables, strikethrough, footnotes — see
//! `src-tauri/src/build/markdown/pipeline.rs`'s `Options` setup).
//!
//! Anything pulldown-cmark emits that the AST hasn't modeled flows through
//! `Block::Other` / `Inline::Other`, which carries the raw HTML so the
//! renderer passes it through unchanged. New variants may be promoted out
//! of `Other` over time as a need is identified.

use serde::{Deserialize, Serialize};

use super::shortcode::Shortcode;
use super::url::Url;

/// Canonical callout kind. Obsidian-dialect aliases canonicalize via
/// [`CalloutKind::from_raw`] (e.g. `tldr`/`summary` → [`CalloutKind::Abstract`]).
/// Unknown kinds fall back to [`CalloutKind::Note`]; the parser logs at
/// trace level (Diagnostic threading is a Phase 4 followup — see
/// `validation::Diagnostic`, today scoped to frontmatter validation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalloutKind {
    Note,
    Abstract,
    Info,
    Todo,
    Tip,
    Success,
    Question,
    Warning,
    Failure,
    Danger,
    Bug,
    Example,
    Quote,
    Important,
    Summary,
    Help,
}

impl CalloutKind {
    /// Canonicalize a raw callout name (case-insensitive) to a
    /// [`CalloutKind`]. Returns `None` if the name is not a recognized
    /// canonical kind or alias.
    ///
    /// Alias table (Obsidian-dialect, per shape-spec § 1):
    /// - `tldr` / `summary` → `Abstract`
    /// - `hint` / `important` → `Tip`
    /// - `check` / `done` → `Success`
    /// - `help` / `faq` → `Question`
    /// - `caution` / `attention` → `Warning`
    /// - `fail` / `missing` → `Failure`
    /// - `error` → `Danger`
    /// - `cite` → `Quote`
    ///
    /// `pending` is also accepted as an alias for `Todo` (used by
    /// SoCiviC Theatre voices.md; carried over from pre-Phase-4 Stage 1
    /// support in `crates/moss-core/src/resolve/callouts.rs`).
    ///
    /// Note: the [`CalloutKind`] enum reserves `Important`, `Summary`,
    /// and `Help` as canonical variants for future Stage 2 use (e.g.
    /// editor-emitted callouts that should not name-clash with the
    /// Obsidian aliases above). Author markdown can't currently produce
    /// these three through `from_raw`; they're reachable only via
    /// programmatic construction.
    pub fn from_raw(raw: &str) -> Option<Self> {
        let lower = raw.to_lowercase();
        let canonical = match lower.as_str() {
            // Canonical kinds (exact match, alias-free names)
            "note" => Self::Note,
            "abstract" => Self::Abstract,
            "info" => Self::Info,
            "todo" => Self::Todo,
            "tip" => Self::Tip,
            "success" => Self::Success,
            "question" => Self::Question,
            "warning" => Self::Warning,
            "failure" => Self::Failure,
            "danger" => Self::Danger,
            "bug" => Self::Bug,
            "example" => Self::Example,
            "quote" => Self::Quote,
            // Obsidian-dialect aliases (shape-spec § 1)
            "tldr" | "summary" => Self::Abstract,
            "hint" | "important" => Self::Tip,
            "check" | "done" => Self::Success,
            "help" | "faq" => Self::Question,
            "caution" | "attention" => Self::Warning,
            "fail" | "missing" => Self::Failure,
            "error" => Self::Danger,
            "cite" => Self::Quote,
            // Legacy alias retained from pre-Phase-4 Stage 1
            // (`crates/moss-core/src/resolve/callouts.rs`). SoCiviC
            // Theatre's voices.md uses `> [!pending]`; map to Todo.
            "pending" => Self::Todo,
            _ => return None,
        };
        Some(canonical)
    }

    /// Slug form used in the rendered `data-type` attribute.
    pub fn as_slug(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Abstract => "abstract",
            Self::Info => "info",
            Self::Todo => "todo",
            Self::Tip => "tip",
            Self::Success => "success",
            Self::Question => "question",
            Self::Warning => "warning",
            Self::Failure => "failure",
            Self::Danger => "danger",
            Self::Bug => "bug",
            Self::Example => "example",
            Self::Quote => "quote",
            Self::Important => "important",
            Self::Summary => "summary",
            Self::Help => "help",
        }
    }

    /// Default display title (capitalized canonical kind) used when the
    /// author wrote `> [!type]` with no inline title text.
    pub fn default_title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Abstract => "Abstract",
            Self::Info => "Info",
            Self::Todo => "Todo",
            Self::Tip => "Tip",
            Self::Success => "Success",
            Self::Question => "Question",
            Self::Warning => "Warning",
            Self::Failure => "Failure",
            Self::Danger => "Danger",
            Self::Bug => "Bug",
            Self::Example => "Example",
            Self::Quote => "Quote",
            Self::Important => "Important",
            Self::Summary => "Summary",
            Self::Help => "Help",
        }
    }
}

/// Foldable callout state. `> [!type]+` → [`Fold::Open`] (foldable,
/// open by default); `> [!type]-` → [`Fold::Closed`] (foldable, closed
/// by default). Non-foldable callouts have `fold: None` on the
/// containing [`Block::Callout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fold {
    Open,
    Closed,
}

/// A block-level AST node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    /// `# Heading` (level 1) through `###### Heading` (level 6).
    Heading {
        level: u8,
        children: Vec<Inline>,
        /// Heading anchor id (slug). Computed by the parser via
        /// [`crate::heading_anchor::obsidian_heading_anchor`].
        id: Option<String>,
    },
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// `> [!type] body` — typed callouts. The `kind` is canonicalized
    /// via [`CalloutKind::from_raw`] (Obsidian-dialect aliases collapse
    /// to the canonical 16-kind set). Foldable callouts (`> [!type]+`
    /// open by default, `> [!type]-` closed) carry the [`Fold`] state;
    /// non-foldable callouts have `fold: None`.
    ///
    /// Phase 4 PR4 extended the shape from `kind: String` to
    /// `kind: CalloutKind` + added `fold: Option<Fold>` and `title: Option<String>`.
    /// Title is the optional inline text following the marker
    /// (`> [!note] My title` → `title: Some("My title")`).
    Callout {
        kind: CalloutKind,
        fold: Option<Fold>,
        title: Option<String>,
        children: Vec<Block>,
    },
    /// `- item` / `1. item`. Each item is a list of blocks (so list items
    /// can carry paragraphs, sub-lists, etc).
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    /// A fenced code block.
    CodeBlock {
        lang: Option<String>,
        value: String,
    },
    /// Markdown table.
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// `> blockquote`
    BlockQuote(Vec<Block>),
    /// A typed shortcode block (`:::name ...args\n body :::`).
    Shortcode(Shortcode),
    /// `<hr>` thematic break.
    ThematicBreak,
    /// Image-only paragraph promoted to a typed figure.
    ///
    /// Detected by the parser's `Tag::Paragraph` arm (Phase 4 PR3,
    /// 2026-05-27): a paragraph that contains exactly one
    /// [`Inline::Image`] modulo whitespace text and line breaks. The
    /// renderer emits `<figure class="moss-image">…<figcaption>…</figcaption></figure>`,
    /// wrapping the image hook's output and appending the caption when
    /// present.
    ///
    /// `image` is constrained by the parser to be an [`Inline::Image`];
    /// the renderer pattern-matches and falls back gracefully if the
    /// variant is anything else.
    ///
    /// `caption` defaults to the image's alt text at parse time. `None`
    /// means "figure wrap but no `<figcaption>`" — reserved for the
    /// empty-alt case (omit caption when there is nothing to read).
    Figure {
        image: Inline,
        caption: Option<Vec<Inline>>,
    },
    /// Compound-link grid cell: the entire cell is a single markdown
    /// link `[inner](url)` whose `inner` is parsed as block-level content
    /// (images, headings, paragraphs, emphasis). The SoCiviC Theatre
    /// pattern: `[![[poster]] ### Title *date* description](/url)`.
    ///
    /// Phase 4 PR4.5 (2026-05-28): added because CommonMark restricts
    /// `Inline::Link.children` to inline-level content; a markdown link
    /// wrapping `### Heading` + paragraphs cannot round-trip through
    /// pulldown-cmark's inline parser. The cell-string-level shape
    /// (`[...](url)` with multi-paragraph inner content) is detected by
    /// `crate::ast::shortcode_extract::parse_grid` BEFORE the cell flows
    /// through `crate::ast::parser::parse`; the matched cell yields a
    /// single-element `vec![Block::LinkCard { url, children }]` with the
    /// inner markdown parsed into typed blocks.
    ///
    /// Render shape (matches today's `render_compound_link_cell` byte
    /// shape):
    /// - External URL (`http(s)://...`): `<a href=URL class="moss-grid-card link-preview" target="_blank" rel="noopener">children</a>`.
    /// - Internal URL: `<a href=URL class="moss-grid-card" data-kind="link">children</a>`.
    LinkCard {
        url: Url,
        children: Vec<Block>,
    },
    /// Escape hatch: anything pulldown-cmark emits that the AST hasn't
    /// modeled. Carries the raw HTML so the renderer passes it through
    /// unchanged.
    Other(String),
}

/// An inline-level AST node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Inline {
    Text(String),
    /// `[content](url "title")`
    Link {
        url: Url,
        title: Option<String>,
        children: Vec<Inline>,
    },
    /// `![alt](src "title")`
    Image {
        src: Url,
        alt: String,
        title: Option<String>,
    },
    /// `*emphasis*`
    Emphasis(Vec<Inline>),
    /// `**strong**`
    Strong(Vec<Inline>),
    /// `` `code` ``
    Code(String),
    /// Hard line break.
    LineBreak,
    /// Escape hatch for unmodeled inline HTML.
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn block_heading_constructable() {
        let b = Block::Heading {
            level: 1,
            children: vec![text("Hello")],
            id: Some("hello".to_string()),
        };
        match b {
            Block::Heading { level, children, id } => {
                assert_eq!(level, 1);
                assert_eq!(children.len(), 1);
                assert_eq!(id.as_deref(), Some("hello"));
            }
            _ => panic!("expected Heading"),
        }
    }

    #[test]
    fn block_paragraph_holds_inlines() {
        let b = Block::Paragraph(vec![text("hi"), Inline::LineBreak, text("there")]);
        match b {
            Block::Paragraph(items) => assert_eq!(items.len(), 3),
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn block_list_each_item_is_block_vec() {
        let b = Block::List {
            ordered: false,
            items: vec![
                vec![Block::Paragraph(vec![text("first")])],
                vec![Block::Paragraph(vec![text("second")])],
            ],
        };
        match b {
            Block::List { ordered, items } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn block_table_two_dim_rows() {
        let b = Block::Table {
            header: vec![vec![text("A")], vec![text("B")]],
            rows: vec![
                vec![vec![text("1")], vec![text("2")]],
                vec![vec![text("3")], vec![text("4")]],
            ],
        };
        match b {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            _ => panic!("expected Table"),
        }
    }

    #[test]
    fn block_other_carries_raw_html() {
        let b = Block::Other("<custom>raw</custom>".to_string());
        match b {
            Block::Other(s) => assert_eq!(s, "<custom>raw</custom>"),
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn block_thematic_break_is_unit_variant() {
        let b = Block::ThematicBreak;
        assert!(matches!(b, Block::ThematicBreak));
    }

    #[test]
    fn block_figure_carries_image_and_optional_caption() {
        // Phase 4 PR3: Block::Figure wraps a single Inline::Image and an
        // optional caption (vector of inlines so emphasis/strong can ride
        // through). Caption defaults to the image's alt text at parse time;
        // None is reserved for the empty-alt case.
        let image = Inline::Image {
            src: Url::resolved("photo.jpg", UrlKind::Asset),
            alt: "A photo".to_string(),
            title: None,
        };
        let b = Block::Figure {
            image: image.clone(),
            caption: Some(vec![text("A photo")]),
        };
        match b {
            Block::Figure { image: img, caption } => {
                assert!(matches!(img, Inline::Image { .. }));
                let cap = caption.expect("caption present");
                assert_eq!(cap.len(), 1);
            }
            _ => panic!("expected Figure"),
        }
    }

    #[test]
    fn block_figure_without_caption_serializes() {
        // Empty-alt case: caption: None means "no figcaption emission."
        let b = Block::Figure {
            image: Inline::Image {
                src: Url::resolved("x.jpg", UrlKind::Asset),
                alt: String::new(),
                title: None,
            },
            caption: None,
        };
        let s = serde_json::to_string(&b).expect("serialize");
        let back: Block = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(b, back);
    }

    #[test]
    fn inline_link_carries_url_and_children() {
        let i = Inline::Link {
            url: Url::unresolved("docs/"),
            title: None,
            children: vec![text("Documentation")],
        };
        match i {
            Inline::Link { url, title, children } => {
                assert!(url.is_unresolved());
                assert!(title.is_none());
                assert_eq!(children.len(), 1);
            }
            _ => panic!("expected Link"),
        }
    }

    #[test]
    fn inline_image_uses_url_for_src() {
        // Per R6: Inline::Image carries Url (not a separate Src type).
        // UrlKind::Asset is the relevant variant after resolution.
        let i = Inline::Image {
            src: Url::resolved("img/cat.jpg", UrlKind::Asset),
            alt: "Cat".to_string(),
            title: None,
        };
        match i {
            Inline::Image { src, alt, title: _ } => {
                let r = src.as_resolved();
                assert_eq!(r.kind, UrlKind::Asset);
                assert_eq!(alt, "Cat");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn inline_emphasis_and_strong_nest() {
        let i = Inline::Strong(vec![Inline::Emphasis(vec![text("nested")])]);
        match i {
            Inline::Strong(children) => match &children[0] {
                Inline::Emphasis(inner) => assert_eq!(inner.len(), 1),
                _ => panic!("expected Emphasis"),
            },
            _ => panic!("expected Strong"),
        }
    }

    #[test]
    fn block_round_trips_through_serde() {
        let original = Block::Heading {
            level: 2,
            children: vec![Inline::Text("Setup".to_string())],
            id: Some("setup".to_string()),
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Block = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }

    // -----------------------------------------------------------------
    // Phase 4 PR4: CalloutKind canonicalization
    // -----------------------------------------------------------------

    #[test]
    fn callout_kind_canonicalizes_canonical_names() {
        assert_eq!(CalloutKind::from_raw("note"), Some(CalloutKind::Note));
        assert_eq!(CalloutKind::from_raw("tip"), Some(CalloutKind::Tip));
        assert_eq!(CalloutKind::from_raw("warning"), Some(CalloutKind::Warning));
        assert_eq!(CalloutKind::from_raw("danger"), Some(CalloutKind::Danger));
        assert_eq!(CalloutKind::from_raw("info"), Some(CalloutKind::Info));
        assert_eq!(CalloutKind::from_raw("todo"), Some(CalloutKind::Todo));
        assert_eq!(CalloutKind::from_raw("success"), Some(CalloutKind::Success));
        assert_eq!(CalloutKind::from_raw("question"), Some(CalloutKind::Question));
        assert_eq!(CalloutKind::from_raw("failure"), Some(CalloutKind::Failure));
        assert_eq!(CalloutKind::from_raw("bug"), Some(CalloutKind::Bug));
        assert_eq!(CalloutKind::from_raw("example"), Some(CalloutKind::Example));
        assert_eq!(CalloutKind::from_raw("quote"), Some(CalloutKind::Quote));
        assert_eq!(CalloutKind::from_raw("abstract"), Some(CalloutKind::Abstract));
    }

    #[test]
    fn callout_kind_canonicalizes_all_obsidian_aliases() {
        // The 8 alias mappings from shape-spec § 1.
        assert_eq!(CalloutKind::from_raw("tldr"), Some(CalloutKind::Abstract));
        assert_eq!(CalloutKind::from_raw("summary"), Some(CalloutKind::Abstract));
        assert_eq!(CalloutKind::from_raw("hint"), Some(CalloutKind::Tip));
        assert_eq!(CalloutKind::from_raw("important"), Some(CalloutKind::Tip));
        assert_eq!(CalloutKind::from_raw("check"), Some(CalloutKind::Success));
        assert_eq!(CalloutKind::from_raw("done"), Some(CalloutKind::Success));
        assert_eq!(CalloutKind::from_raw("help"), Some(CalloutKind::Question));
        assert_eq!(CalloutKind::from_raw("faq"), Some(CalloutKind::Question));
        assert_eq!(CalloutKind::from_raw("caution"), Some(CalloutKind::Warning));
        assert_eq!(CalloutKind::from_raw("attention"), Some(CalloutKind::Warning));
        assert_eq!(CalloutKind::from_raw("fail"), Some(CalloutKind::Failure));
        assert_eq!(CalloutKind::from_raw("missing"), Some(CalloutKind::Failure));
        assert_eq!(CalloutKind::from_raw("error"), Some(CalloutKind::Danger));
        assert_eq!(CalloutKind::from_raw("cite"), Some(CalloutKind::Quote));
        // Legacy alias for SoCiviC Theatre's `> [!pending]` syntax.
        assert_eq!(CalloutKind::from_raw("pending"), Some(CalloutKind::Todo));
    }

    #[test]
    fn callout_kind_is_case_insensitive() {
        assert_eq!(CalloutKind::from_raw("NOTE"), Some(CalloutKind::Note));
        assert_eq!(CalloutKind::from_raw("Warning"), Some(CalloutKind::Warning));
        assert_eq!(CalloutKind::from_raw("TLDR"), Some(CalloutKind::Abstract));
    }

    #[test]
    fn callout_kind_unknown_returns_none() {
        assert_eq!(CalloutKind::from_raw("xyz"), None);
        assert_eq!(CalloutKind::from_raw(""), None);
        assert_eq!(CalloutKind::from_raw("not-a-kind"), None);
    }

    #[test]
    fn callout_kind_slug_matches_canonical_name() {
        assert_eq!(CalloutKind::Note.as_slug(), "note");
        assert_eq!(CalloutKind::Abstract.as_slug(), "abstract");
        assert_eq!(CalloutKind::Warning.as_slug(), "warning");
        assert_eq!(CalloutKind::Danger.as_slug(), "danger");
    }

    #[test]
    fn callout_kind_default_title_is_capitalized() {
        assert_eq!(CalloutKind::Note.default_title(), "Note");
        assert_eq!(CalloutKind::Warning.default_title(), "Warning");
        assert_eq!(CalloutKind::Abstract.default_title(), "Abstract");
    }

    #[test]
    fn block_callout_round_trips_through_serde() {
        let original = Block::Callout {
            kind: CalloutKind::Warning,
            fold: Some(Fold::Open),
            title: Some("Hey".to_string()),
            children: vec![Block::Paragraph(vec![Inline::Text("body".into())])],
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Block = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn inline_link_with_resolved_url_round_trips() {
        let original = Inline::Link {
            url: Url::resolved("../docs/", UrlKind::Wikilink),
            title: Some("Docs".to_string()),
            children: vec![Inline::Text("see".to_string())],
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Inline = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }
}
