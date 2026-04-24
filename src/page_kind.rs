//! Classification of moss page/source-file kinds.
//!
//! moss recognizes exactly three kinds of page-producing entities in a
//! source tree. This enum is the canonical way to tell them apart — do
//! NOT use `is_index`, filename checks, or extension sniffing at call
//! sites. The kind is set once at ingestion (for markdown files) or
//! synthesis (for asset pages) and read everywhere else.
//!
//! # Variants
//!
//! * [`PageKind::Article`] — a markdown file that is not a folder index.
//!   Authored prose content. Listed in children at any depth.
//! * [`PageKind::Folder`] — a markdown file recognized by
//!   [`crate::home::is_home_file`]. Represents the directory it lives in
//!   (e.g. `文字/文字.md`, `docs/index.md`, `blog/readme.md`). Listed in
//!   children only at depth 1 (at depth `all`, its descendants are listed
//!   directly).
//! * [`PageKind::Asset`] — a synthetic page generated for a non-markdown
//!   file (e.g. per-image pages in image-only folders). Never listed as
//!   a child regardless of depth.
//!
//! # Why an enum, not a bool
//!
//! The previous model used `is_index: bool` and inferred "article vs.
//! asset" from context. That made the children-listing filter impossible
//! to get right — image pages passed `!is_index` and leaked into listings
//! on sites using `children_depth: all`. See `moss/docs/page-kinds.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    /// Markdown file, not a folder index.
    Article,
    /// Markdown file recognized as a folder index by
    /// [`crate::home::is_home_file`].
    Folder,
    /// Synthetic page for a non-markdown asset (per-image pages, etc.).
    Asset,
}

impl PageKind {
    /// True when pages of this kind appear in children listings at
    /// `children_depth: all`.
    pub fn is_listable_at_depth_all(self) -> bool {
        matches!(self, PageKind::Article)
    }

    /// True when pages of this kind appear in children listings at
    /// `children_depth: direct`.
    pub fn is_listable_at_depth_direct(self) -> bool {
        matches!(self, PageKind::Article | PageKind::Folder)
    }
}

impl Default for PageKind {
    fn default() -> Self {
        PageKind::Article
    }
}
