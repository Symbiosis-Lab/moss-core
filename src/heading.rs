//! Article heading rule — single source of truth for the auto-injected
//! `<h1 class="moss-article-title">` and the editor's pinned heading element.
//!
//! Both consumers — the build pipeline (`src-tauri/src/build/markdown/pipeline.rs`)
//! and the editor command `compute_heading_state`
//! (`src-tauri/src/editor/commands.rs`) — feed the same inputs into [`compute`]
//! and act on the same answer. Without this module the two paths silently
//! desync the next time the rule changes.
//!
//! The rule:
//!   - Markdown files only.
//!   - Index/folder pages never get the auto-injected H1.
//!   - Frontmatter `title:` drives the heading text and source:
//!     - missing → filename-mode (text from filename, source = Filename)
//!     - non-empty → title-mode (text from title:, source = Title, visible)
//!     - empty `""` or whitespace-only → title-mode + invisible (explicit no-heading)
//!   - A `:::hero` block at the top of the body owns the title slot —
//!     no auto-injection regardless of title.
//!
//! See `docs/architecture/title-rendering.md`.

use crate::home;

/// Resolved heading state for a single page.
///
/// `visible` is the gate the build pipeline checks before injecting an
/// `<h1 class="moss-article-title">` and the editor checks before rendering
/// its pinned heading element. `source` tells the editor where `text` came
/// from so it can route heading-element commits between rename (Filename)
/// and title-update (Title).
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeadingState {
    /// Whether the article heading should render.
    pub visible: bool,
    /// The visible heading text (resolved through title or filename).
    pub text: String,
    /// Where `text` was sourced from.
    pub source: HeadingSource,
}

/// Where the resolved heading text came from. New variants are added when a
/// new origin is introduced; consumers handle them exhaustively.
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HeadingSource {
    /// Text derived from filename (or parent folder for index notes).
    Filename,
    /// Text from frontmatter `title:`. `Some("")` and `Some("   ")` count as
    /// Title source with empty text — `visible` will be false in that case.
    Title,
}

/// Inputs to the heading rule.
#[derive(Debug, Clone, Copy)]
pub struct HeadingInputs<'a> {
    pub file_path: &'a str,
    /// Frontmatter `title:` value verbatim. `None` is "field missing"
    /// (filename-mode); `Some("")`, `Some("   ")` is "explicitly empty"
    /// (Title source, invisible).
    pub frontmatter_title: Option<&'a str>,
    /// Raw markdown body (post-frontmatter). Used to detect a leading
    /// `:::hero` block — when present, the hero owns the heading slot and
    /// the auto-injected H1 is suppressed regardless of title.
    pub body_markdown: &'a str,
    /// The project's root folder name (the user's vault directory basename).
    /// Used as a fallback for self-named-folder-note detection when the
    /// file is at the project root: `<root>/<root>.md` and `<root>/index.md`
    /// both render the project's homepage, but `Path::parent()` returns
    /// the empty path for root-level files, so the path-based check can't
    /// see the folder name. Editor and pipeline both pass the basename of
    /// the open project folder. Pass `None` if unknown (filename
    /// detection still catches index.md / README.md / language-suffixed
    /// indexes).
    pub root_folder_name: Option<&'a str>,
    /// `true` iff the file is promoted to its folder's home via
    /// `translationKey: home` (issue #587). Pipeline-only input; editor
    /// passes `false`.
    pub is_translation_home: bool,
    /// `true` iff the file is a layout-slot source (e.g. root `footer.md`)
    /// rather than an article. Slot files are embedded as fragments into a
    /// surrounding layout; the auto-injected `<h1 class="moss-article-title">`
    /// would render as an unwanted heading inside that fragment. PR7b
    /// (moss#599) replaces the pre-2026-05-28 frontmatter-synthesis hack
    /// (`title: ""` injected at the call site to drive `empty_title=true`)
    /// with this structural input. Editor passes `false`.
    pub slot_only: bool,
}

/// Compute just the visible heading text for a file path. Used by callers
/// that need the text before the full state.
pub fn filename_text(file_path: &str) -> String {
    let path = std::path::Path::new(file_path);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());
    let is_folder_note = home::is_index_stem(stem)
        || parent_name.is_some_and(|p| p.eq_ignore_ascii_case(stem));
    let source_name = if is_folder_note {
        parent_name.unwrap_or(stem)
    } else {
        stem
    };
    source_name.replace('-', " ").replace('_', " ")
}

/// Whether the body begins with a `:::hero` block, after any leading blank
/// lines. The first non-blank content line must be `:::hero` followed
/// optionally by attributes/whitespace.
pub fn body_starts_with_hero(body_markdown: &str) -> bool {
    body_markdown
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            let trimmed = line.trim_start();
            trimmed == ":::hero"
                || trimmed.starts_with(":::hero ")
                || trimmed.starts_with(":::hero\t")
        })
        .unwrap_or(false)
}

/// Compute the full heading state for a page.
pub fn compute(input: HeadingInputs<'_>) -> HeadingState {
    let path = std::path::Path::new(input.file_path);

    // Resolve text + source from frontmatter.title before all other rules.
    // Some(_) → Title source (empty allowed); None → Filename source.
    let (text, source) = match input.frontmatter_title {
        Some(t) => (t.trim().to_string(), HeadingSource::Title),
        None => (filename_text(input.file_path), HeadingSource::Filename),
    };

    let is_markdown = matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .as_deref(),
        Some("md") | Some("mdx") | Some("markdown")
    );

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // Resolve parent folder name. For nested files (`recipes/recipes.md`)
    // we pull from the path. For ROOT-level files (`刘果.md` in a vault
    // named `刘果`) Path::parent() returns the empty path with no
    // file_name, so we fall back to `root_folder_name` — letting
    // self-named-home detection work at the project root.
    let parent_from_path = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let parent_name = if parent_from_path.is_empty() {
        input.root_folder_name.unwrap_or("")
    } else {
        parent_from_path
    };
    let filename_lower = stem.to_lowercase();
    let is_index_file =
        home::is_home_file(&filename_lower, parent_name) || input.is_translation_home;

    let empty_title = source == HeadingSource::Title && text.is_empty();
    let hero_at_top = body_starts_with_hero(input.body_markdown);

    let visible =
        is_markdown && !is_index_file && !empty_title && !hero_at_top && !input.slot_only;

    HeadingState { visible, text, source }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(file_path: &'a str, frontmatter_title: Option<&'a str>) -> HeadingInputs<'a> {
        HeadingInputs {
            file_path,
            frontmatter_title,
            body_markdown: "",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        }
    }

    // ── filename_text ────────────────────────────────────────────────

    #[test]
    fn text_article_uses_filename() {
        assert_eq!(filename_text("posts/my-first-post.md"), "my first post");
    }

    #[test]
    fn text_underscore_normalizes_to_space() {
        assert_eq!(filename_text("posts/my_first_post.md"), "my first post");
    }

    #[test]
    fn text_index_uses_parent_folder() {
        assert_eq!(filename_text("site/index.md"), "site");
    }

    #[test]
    fn text_readme_uses_parent_folder() {
        assert_eq!(filename_text("docs/README.md"), "docs");
    }

    #[test]
    fn text_self_named_folder_note_uses_parent() {
        assert_eq!(filename_text("recipes/recipes.md"), "recipes");
    }

    #[test]
    fn text_root_level_no_parent() {
        assert_eq!(filename_text("about.md"), "about");
    }

    #[test]
    fn text_cjk_filename_preserved() {
        assert_eq!(filename_text("文字/民歌.md"), "民歌");
    }

    #[test]
    fn text_cjk_index_uses_parent() {
        assert_eq!(filename_text("文字/index.md"), "文字");
    }

    // ── compute: visibility + filename mode ──────────────────────────

    #[test]
    fn visible_for_article_md() {
        let s = compute(inputs("posts/my-first-post.md", None));
        assert!(s.visible);
        assert_eq!(s.text, "my first post");
        assert!(matches!(s.source, HeadingSource::Filename));
    }

    #[test]
    fn hidden_for_index_file() {
        let s = compute(inputs("site/index.md", None));
        assert!(!s.visible);
        assert_eq!(s.text, "site");
    }

    #[test]
    fn hidden_for_readme() {
        let s = compute(inputs("docs/README.md", None));
        assert!(!s.visible);
        assert_eq!(s.text, "docs");
    }

    #[test]
    fn hidden_for_self_named_folder_note() {
        let s = compute(inputs("recipes/recipes.md", None));
        assert!(!s.visible);
        assert_eq!(s.text, "recipes");
    }

    #[test]
    fn hidden_for_root_self_named_home_with_root_folder_name() {
        // Regression: a vault opened at `刘果/`, with `刘果.md` at the project
        // root, must be detected as the home file. Path::parent() returns
        // the empty path (no file_name), so without `root_folder_name` the
        // path-based check misses the self-named case.
        let s = compute(HeadingInputs {
            file_path: "刘果.md",
            frontmatter_title: None,
            body_markdown: "",
            root_folder_name: Some("刘果"),
            is_translation_home: false,
            slot_only: false,
        });
        assert!(!s.visible, "root-level self-named home file must hide H1");
    }

    #[test]
    fn root_index_md_still_hidden_without_root_folder_name() {
        // Compatibility: even when the caller passes None for
        // root_folder_name, recognized index stems (index.md / README.md)
        // at the root are still detected via is_index_stem.
        let s = compute(HeadingInputs {
            file_path: "index.md",
            frontmatter_title: None,
            body_markdown: "",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        });
        assert!(!s.visible);
    }

    #[test]
    fn hidden_for_non_markdown() {
        let s = compute(inputs("assets/style.css", None));
        assert!(!s.visible);
    }

    #[test]
    fn visible_for_mdx() {
        let s = compute(inputs("posts/article.mdx", None));
        assert!(s.visible);
        assert_eq!(s.text, "article");
    }

    #[test]
    fn visible_for_root_level_article() {
        let s = compute(inputs("about.md", None));
        assert!(s.visible);
        assert_eq!(s.text, "about");
    }

    #[test]
    fn visible_for_cjk_article() {
        let s = compute(inputs("文字/民歌.md", None));
        assert!(s.visible);
        assert_eq!(s.text, "民歌");
    }

    #[test]
    fn hidden_for_cjk_index() {
        let s = compute(inputs("文字/index.md", None));
        assert!(!s.visible);
        assert_eq!(s.text, "文字");
    }

    // ── compute: source enum / title mode ────────────────────────────

    #[test]
    fn source_is_title_when_frontmatter_title_set() {
        let s = compute(inputs("posts/article.md", Some("Custom")));
        assert_eq!(s.text, "Custom");
        assert!(matches!(s.source, HeadingSource::Title));
        assert!(s.visible);
    }

    #[test]
    fn source_is_filename_when_title_absent() {
        let s = compute(inputs("posts/article.md", None));
        assert!(matches!(s.source, HeadingSource::Filename));
        assert_eq!(s.text, "article");
        assert!(s.visible);
    }

    #[test]
    fn empty_title_produces_invisible_state() {
        let s = compute(inputs("posts/article.md", Some("")));
        assert!(matches!(s.source, HeadingSource::Title));
        assert_eq!(s.text, "");
        assert!(!s.visible, "title: \"\" suppresses the auto-injected H1");
    }

    #[test]
    fn whitespace_title_produces_invisible_state() {
        let s = compute(inputs("posts/article.md", Some("   ")));
        assert!(matches!(s.source, HeadingSource::Title));
        assert_eq!(s.text, "");
        assert!(!s.visible);
    }

    #[test]
    fn title_overrides_index_visibility_unchanged() {
        let s = compute(inputs("site/index.md", Some("Welcome")));
        assert!(!s.visible, "index pages still don't auto-inject");
        assert!(matches!(s.source, HeadingSource::Title));
        assert_eq!(s.text, "Welcome");
    }

    #[test]
    fn title_text_is_trimmed() {
        let s = compute(inputs("posts/article.md", Some("  Custom  ")));
        assert_eq!(s.text, "Custom");
        assert!(s.visible);
    }

    // ── compute: hero-from-body ──────────────────────────────────────

    #[test]
    fn hero_at_top_hides_heading_when_title_absent() {
        let s = compute(HeadingInputs {
            file_path: "posts/article.md",
            frontmatter_title: None,
            body_markdown: ":::hero\nimage: x.jpg\n:::\n\nBody.",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        });
        assert!(!s.visible);
        assert_eq!(s.text, "article");
    }

    #[test]
    fn hero_at_top_hides_heading_when_title_set() {
        let s = compute(HeadingInputs {
            file_path: "posts/article.md",
            frontmatter_title: Some("Custom"),
            body_markdown: ":::hero\n:::\n\nBody.",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        });
        assert!(!s.visible, "hero ownership trumps title presence");
        assert_eq!(s.text, "Custom");
    }

    #[test]
    fn hero_only_detected_at_top_not_mid_body() {
        let s = compute(HeadingInputs {
            file_path: "posts/article.md",
            frontmatter_title: None,
            body_markdown: "Some intro paragraph.\n\n:::hero\n:::",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        });
        assert!(s.visible, "hero anywhere but at top does not own heading");
    }

    #[test]
    fn hero_detection_skips_leading_blank_lines() {
        let s = compute(HeadingInputs {
            file_path: "posts/article.md",
            frontmatter_title: None,
            body_markdown: "\n\n\n:::hero\n:::",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: false,
        });
        assert!(!s.visible, "leading blanks before :::hero still count as 'at top'");
    }

    // ── compute: translation-home override ───────────────────────────

    #[test]
    fn hidden_when_translation_home() {
        let s = compute(HeadingInputs {
            file_path: "posts/article.md",
            frontmatter_title: None,
            body_markdown: "",
            root_folder_name: None,
            is_translation_home: true,
            slot_only: false,
        });
        assert!(!s.visible);
    }

    // ── compute: slot_only override ──────────────────────────────────

    #[test]
    fn slot_only_hides_heading_regardless_of_title() {
        // PR7b (moss#599): `footer.md` flows through the normal pipeline
        // with `slot_only = true`. The auto-injected H1 must be suppressed
        // even when the author writes `title: "Custom"` in the
        // frontmatter — the rendered HTML lands inside a `<footer>` slot,
        // and an article-level heading there is structurally wrong.
        let s = compute(HeadingInputs {
            file_path: "footer.md",
            frontmatter_title: Some("Custom"),
            body_markdown: "[link](https://example.com)",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: true,
        });
        assert!(
            !s.visible,
            "slot_only must suppress the auto-injected H1 even when title: is set"
        );
        // The text is preserved (chrome label / RSS still reads it).
        assert_eq!(s.text, "Custom");
    }

    #[test]
    fn slot_only_hides_heading_when_title_absent() {
        let s = compute(HeadingInputs {
            file_path: "footer.md",
            frontmatter_title: None,
            body_markdown: "Studio · 2026",
            root_folder_name: None,
            is_translation_home: false,
            slot_only: true,
        });
        assert!(!s.visible);
    }

    // ── body_starts_with_hero helper ─────────────────────────────────

    #[test]
    fn body_starts_with_hero_basic() {
        assert!(body_starts_with_hero(":::hero\n:::"));
        assert!(body_starts_with_hero("\n\n:::hero\nimage: x\n:::"));
        assert!(body_starts_with_hero(":::hero attr=value\n:::"));
        assert!(!body_starts_with_hero("# Heading\n:::hero\n:::"));
        assert!(!body_starts_with_hero("Some prose first.\n\n:::hero\n:::"));
        assert!(!body_starts_with_hero(""));
        assert!(!body_starts_with_hero("\n\n"));
    }
}
