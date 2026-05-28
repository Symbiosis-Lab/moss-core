//! Typed shortcode AST nodes.
//!
//! Each shortcode is a closed enum variant with fully-typed arguments.
//! Variants land per-shortcode in Phase B (one variant per migration
//! commit) of the typed-AST migration.
//!
//! Migration order (Phase B): Subscribe, Buttons, Gallery, Hero, Grid, Recent.

use serde::{Deserialize, Serialize};

use super::url::Url;

/// A typed shortcode block.
///
/// Variants:
/// - [`Shortcode::Subscribe`] — inline subscribe form (description + button)
/// - [`Shortcode::Buttons`] — list of action buttons with markdown links
/// - [`Shortcode::Gallery`] — image gallery with optional column count
/// - [`Shortcode::Hero`] — full-width hero section with media + overlay
/// - [`Shortcode::Grid`] — flexible multi-cell layout
/// - [`Shortcode::Recent`] — recent-posts query with fallback markdown
///
/// Phase B migrations add one variant per commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Shortcode {
    /// `:::subscribe` — inline newsletter signup form.
    ///
    /// Configuration is via attributes (`placeholder`, `button`); body
    /// must be empty under the unified grammar. Description text and
    /// any framing prose live in the surrounding markdown.
    Subscribe(SubscribeShortcode),
    /// `:::buttons {.classname}` — list of action buttons.
    ///
    /// Body is one markdown link per line (`[text](url)`). The first
    /// button gets the primary class; subsequent buttons get secondary.
    /// Optional `{.classname}` extra classes attach to the wrapping div.
    ///
    /// URLs flow through [`Url::Unresolved`] at parse time;
    /// [`crate::ast::visit::visit_urls_mut`] (or src-tauri's
    /// `apply_typed_shortcodes`) classifies them into [`Url::Resolved`]
    /// before rendering. The resolver-bypass class is closed by
    /// construction: `RenderHooks::render_shortcode` reads `Url::Resolved`,
    /// so a missing visitor is a debug-time crash.
    Buttons(ButtonsShortcode),
    /// `:::gallery N {.classname}` — image gallery with optional columns.
    ///
    /// `N` (positional integer) sets `--gallery-columns` CSS variable.
    /// Body is one image reference per line: `![alt](path)`, bare
    /// `path.jpg`, or `path|attrs` for media attributes (passed through
    /// to the renderer's inline style).
    Gallery(GalleryShortcode),
    /// `:::hero {image=path}` — full-width hero section with media + overlay.
    ///
    /// New grammar: `image` attribute carries the path. Backward-compat:
    /// when `image` is absent, the extractor scans the first non-empty
    /// body line for a media reference (`![[path]]`, `![alt](path)`, or
    /// bare media filename).
    ///
    /// The pipeline hoists the rendered hero HTML into the article
    /// template's hero slot — it does NOT render inline.
    Hero(HeroShortcode),
    /// `:::grid {cols=N}` or `:::grid N` — flexible multi-cell layout.
    ///
    /// Cells are split on `+++` (new grammar) or `---` (legacy moss-releases
    /// backward-compat — Step 3 of #613 rewrites these to `+++`). Each cell
    /// stores its raw markdown source; the renderer is responsible for any
    /// nested-shortcode extraction and markdown processing per cell.
    Grid(GridShortcode),
    /// `:::recent since=... last=... count=...` — list of recent posts
    /// scoped to the page's top-level folder (its scope).
    ///
    /// Body (between the opening and closing `:::`) is reserved for
    /// fallback content rendered when the query returns zero matches.
    /// Empty body means no fallback (the shortcode renders nothing).
    Recent(RecentShortcode),
}

/// Arguments for [`Shortcode::Subscribe`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeShortcode {
    /// Optional override for the email input's placeholder text.
    pub placeholder: Option<String>,
    /// Optional override for the submit button label.
    pub button: Option<String>,
}

/// Arguments for [`Shortcode::Buttons`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonsShortcode {
    /// Extra CSS classes for the wrapping `<div>` (from `{.foo .bar}`).
    pub classes: String,
    /// Each button's text + URL. The first item renders as primary, the
    /// rest as secondary. Empty list = the shortcode renders nothing.
    pub items: Vec<ButtonItem>,
}

/// One button in a [`ButtonsShortcode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonItem {
    /// Display text inside the `<a>` tag.
    pub text: String,
    /// Click target. Author input as parsed; flows through
    /// [`crate::ast::visit::visit_urls_mut`] before reaching the renderer.
    pub url: Url,
}

/// Arguments for [`Shortcode::Gallery`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalleryShortcode {
    /// Optional column count for `--gallery-columns` CSS variable.
    pub columns: Option<u32>,
    /// Extra CSS classes for the wrapping `<div>` (from `{.foo .bar}`).
    pub classes: String,
    /// Each gallery image's src + alt + media attrs.
    pub items: Vec<GalleryItem>,
    /// Spec § P9 width attribute: `body | wide | page | screen` (with
    /// `full` aliased to `screen`). `None` means the author did not set
    /// a width — the emitter omits `data-width` so the HTML stays sparse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
}

/// One image in a [`GalleryShortcode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalleryItem {
    /// Image source URL. Flows through resolver before rendering.
    pub src: Url,
    /// Alt text (from `![alt](...)` syntax). Empty if author used bare path.
    pub alt: String,
    /// Pipe-suffix media attributes verbatim (e.g. "cover top",
    /// "1.5:1 contain"). Empty if no pipe in the source.
    /// The renderer parses this via `moss_core::media::parse_media_attrs`.
    pub attrs: String,
}

/// Arguments for [`Shortcode::Grid`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridShortcode {
    /// Column count. Defaults to 1 when neither positional nor `cols=`
    /// attribute is provided.
    pub columns: u32,
    /// Optional ratio string like `"1:2"` or `"1:1:2"`. When present,
    /// the renderer emits `style="grid-template-columns:1fr 2fr"` etc.
    /// `cols=1:2:3` is equivalent to setting both `columns` (count = 3)
    /// and `ratio` to `"1:2:3"`.
    pub ratio: Option<String>,
    /// Extra CSS classes for the wrapping `<div>` (from `{.foo .bar}`).
    pub classes: String,
    /// Each cell's raw markdown source. The renderer processes each cell
    /// through the markdown pipeline (including any nested typed
    /// shortcodes such as `::::buttons` inside a `:::grid` cell).
    pub cells: Vec<String>,
    /// Spec § P9 width attribute: `body | wide | page | screen` (with
    /// `full` aliased to `screen`). `None` means the author did not set
    /// a width — the emitter omits `data-width` so the HTML stays sparse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
}

/// Arguments for [`Shortcode::Hero`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeroShortcode {
    /// Image source URL. `None` if neither the `image` attribute nor the
    /// first body line provided one — the renderer emits a section with
    /// no `<img>` in that case. Flows through resolver before rendering.
    pub image: Option<Url>,
    /// Pipe-suffix media attributes verbatim (e.g. "cover top",
    /// "1.5:1 contain"). Empty if no pipe in the source.
    pub attrs: String,
    /// Extra CSS classes for the wrapping `<section>` (from `{.foo .bar}`).
    pub classes: String,
    /// Markdown source for the overlay content. Renderer processes this
    /// via the surrounding markdown pipeline.
    pub overlay_markdown: String,
    /// Spec § P9 width attribute: `body | wide | page | screen` (with
    /// `full` aliased to `screen`). `None` means the author did not set
    /// a width — the emitter omits `data-width` so the HTML stays sparse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
}

/// Arguments for [`Shortcode::Recent`].
///
/// Parameters parsed at shortcode-extract time; the query runs at render
/// time against the full post set. Renderer lives in
/// `src-tauri/src/build/markdown/recent.rs`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentShortcode {
    /// `since="YYYY-MM-DD"` — posts on or after this date. Stored as the
    /// raw string here; the rendering layer parses it into a DateTime.
    /// Mutually compatible with `last` — both set the cutoff, later wins.
    pub since: Option<String>,
    /// `last="week" | "month" | "Nd"` — relative window. The renderer
    /// converts this to a duration and subtracts from now.
    pub last: Option<String>,
    /// `count="N"` — cap at N most recent posts. The renderer applies a
    /// default of 10 when unset.
    pub count: Option<u32>,
    /// Body content rendered as fallback when zero posts match. Empty
    /// string means no fallback. Lives in the AST so the renderer doesn't
    /// need to re-read the source.
    pub fallback_markdown: String,
}

/// Identifier for a shortcode kind, used for AST queries (e.g.
/// `has_shortcode(&doc, ShortcodeKind::Subscribe)` to gate feature
/// detection without scanning source files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcodeKind {
    Subscribe,
    Buttons,
    Gallery,
    Hero,
    Grid,
    Recent,
}

impl Shortcode {
    /// Return the [`ShortcodeKind`] of this shortcode.
    pub fn kind(&self) -> ShortcodeKind {
        match self {
            Shortcode::Subscribe(_) => ShortcodeKind::Subscribe,
            Shortcode::Buttons(_) => ShortcodeKind::Buttons,
            Shortcode::Gallery(_) => ShortcodeKind::Gallery,
            Shortcode::Hero(_) => ShortcodeKind::Hero,
            Shortcode::Grid(_) => ShortcodeKind::Grid,
            Shortcode::Recent(_) => ShortcodeKind::Recent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcode_kind_variants_are_distinct() {
        let kinds = [
            ShortcodeKind::Subscribe,
            ShortcodeKind::Buttons,
            ShortcodeKind::Gallery,
            ShortcodeKind::Hero,
            ShortcodeKind::Grid,
            ShortcodeKind::Recent,
        ];
        let unique: std::collections::HashSet<_> = kinds.iter().collect();
        assert_eq!(unique.len(), kinds.len());
    }

    #[test]
    fn shortcode_kind_round_trips_through_serde() {
        for kind in [
            ShortcodeKind::Subscribe,
            ShortcodeKind::Buttons,
            ShortcodeKind::Gallery,
            ShortcodeKind::Hero,
            ShortcodeKind::Grid,
            ShortcodeKind::Recent,
        ] {
            let s = serde_json::to_string(&kind).expect("serialize");
            let back: ShortcodeKind = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn subscribe_kind_method_returns_subscribe() {
        let sc = Shortcode::Subscribe(SubscribeShortcode::default());
        assert_eq!(sc.kind(), ShortcodeKind::Subscribe);
    }

    #[test]
    fn subscribe_with_placeholder_and_button() {
        let sc = Shortcode::Subscribe(SubscribeShortcode {
            placeholder: Some("you@example.com".to_string()),
            button: Some("Subscribe".to_string()),
        });
        match &sc {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.placeholder.as_deref(), Some("you@example.com"));
                assert_eq!(args.button.as_deref(), Some("Subscribe"));
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn subscribe_default_has_none_placeholder_and_button() {
        let args = SubscribeShortcode::default();
        assert!(args.placeholder.is_none());
        assert!(args.button.is_none());
    }

    #[test]
    fn subscribe_round_trips_through_serde() {
        let sc = Shortcode::Subscribe(SubscribeShortcode {
            placeholder: Some("p".to_string()),
            button: Some("b".to_string()),
        });
        let s = serde_json::to_string(&sc).expect("serialize");
        let back: Shortcode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(sc, back);
    }

    // ---- Buttons ----

    #[test]
    fn buttons_kind_method_returns_buttons() {
        let sc = Shortcode::Buttons(ButtonsShortcode::default());
        assert_eq!(sc.kind(), ShortcodeKind::Buttons);
    }

    #[test]
    fn buttons_items_carry_unresolved_urls() {
        let sc = Shortcode::Buttons(ButtonsShortcode {
            classes: String::new(),
            items: vec![
                ButtonItem {
                    text: "Docs".to_string(),
                    url: Url::unresolved("docs/"),
                },
                ButtonItem {
                    text: "GitHub".to_string(),
                    url: Url::unresolved("https://github.com"),
                },
            ],
        });
        match &sc {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert!(args.items[0].url.is_unresolved());
                assert!(args.items[1].url.is_unresolved());
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn buttons_default_has_no_items() {
        let args = ButtonsShortcode::default();
        assert!(args.items.is_empty());
        assert!(args.classes.is_empty());
    }

    #[test]
    fn buttons_round_trips_through_serde() {
        let sc = Shortcode::Buttons(ButtonsShortcode {
            classes: "primary".to_string(),
            items: vec![ButtonItem {
                text: "Go".to_string(),
                url: Url::unresolved("/x"),
            }],
        });
        let s = serde_json::to_string(&sc).expect("serialize");
        let back: Shortcode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(sc, back);
    }

    // ---- Gallery ----

    #[test]
    fn gallery_kind_method_returns_gallery() {
        let sc = Shortcode::Gallery(GalleryShortcode::default());
        assert_eq!(sc.kind(), ShortcodeKind::Gallery);
    }

    #[test]
    fn gallery_items_carry_unresolved_urls() {
        let sc = Shortcode::Gallery(GalleryShortcode {
            columns: Some(3),
            classes: String::new(),
            items: vec![
                GalleryItem {
                    src: Url::unresolved("a.jpg"),
                    alt: "A".to_string(),
                    attrs: String::new(),
                },
                GalleryItem {
                    src: Url::unresolved("b.jpg"),
                    alt: "B".to_string(),
                    attrs: "cover top".to_string(),
                },
            ],
            width: None,
        });
        match &sc {
            Shortcode::Gallery(args) => {
                assert_eq!(args.columns, Some(3));
                assert_eq!(args.items.len(), 2);
                assert!(args.items[0].src.is_unresolved());
                assert_eq!(args.items[1].attrs, "cover top");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn gallery_default_no_columns_no_items() {
        let args = GalleryShortcode::default();
        assert!(args.columns.is_none());
        assert!(args.items.is_empty());
        assert!(args.classes.is_empty());
    }

    #[test]
    fn gallery_round_trips_through_serde() {
        let sc = Shortcode::Gallery(GalleryShortcode {
            columns: Some(4),
            classes: "showcase".to_string(),
            items: vec![GalleryItem {
                src: Url::unresolved("p.png"),
                alt: "Photo".to_string(),
                attrs: "1:1 contain".to_string(),
            }],
            width: None,
        });
        let s = serde_json::to_string(&sc).expect("serialize");
        let back: Shortcode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(sc, back);
    }

    // ---- Recent ----

    #[test]
    fn recent_kind_method_returns_recent() {
        let sc = Shortcode::Recent(RecentShortcode::default());
        assert_eq!(sc.kind(), ShortcodeKind::Recent);
    }

    #[test]
    fn recent_default_has_none_params_empty_fallback() {
        let args = RecentShortcode::default();
        assert!(args.since.is_none());
        assert!(args.last.is_none());
        assert!(args.count.is_none());
        assert!(args.fallback_markdown.is_empty());
    }

    #[test]
    fn recent_round_trips_through_serde() {
        let sc = Shortcode::Recent(RecentShortcode {
            since: Some("2026-04-01".to_string()),
            last: None,
            count: Some(5),
            fallback_markdown: "_No posts yet._".to_string(),
        });
        let s = serde_json::to_string(&sc).expect("serialize");
        let back: Shortcode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(sc, back);
    }
}
