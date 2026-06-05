//! The single reference classifier shared by build + editor (asset/file-embed/
//! folder kinds). Pure; indexes injected via ReferenceContext. Page-Link
//! emission is out of scope here (the build keeps relative_pretty_url/page_map);
//! Link is classify-only. Named `classify_reference` to avoid colliding with
//! `fuzzy_path::resolve_reference` (the [[note]]/ContentGraph resolver).

use crate::resolve::asset_class::{AssetIndex, AssetProvenance};
use crate::resolve::embed_renderer::Sizing;
use crate::resolve::folder_class::FolderIndex;
use crate::resolve::link_class::UrlIndex;

#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "data")]
pub enum ReferenceKind {
    Link { anchor: Option<String> },
    Image,
    Iframe,
    Pdf,
    Video,
    Audio,
    Model,
    FolderListing,
    FolderIndexIframe,
    Transclusion,
    Notebook,
    Table,
    External { url: String },
    Anchor,
    Ambiguous,
    NotFound,
}

/// Index handles a classify call needs. Bundled so the signature stays small
/// and a future index can be added without re-touching every caller.
pub struct ReferenceContext<'a> {
    pub assets: &'a dyn AssetIndex,
    pub folders: &'a dyn FolderIndex,
    /// Link arm only; the build supplies a graph-backed impl in sub-project #4.
    /// For unit #1+#2 a Link result is classify-only and this may be a no-op.
    pub urls: &'a dyn UrlIndex,
}

#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedReference {
    pub kind: ReferenceKind,
    /// Root-relative SOURCE path (real case) for file/folder kinds; None for
    /// Link/External/Anchor/Ambiguous/NotFound.
    pub target_path: Option<String>,
    pub size: Option<Sizing>,
    pub provenance: Option<AssetProvenance>,
    /// Human-readable resolution note (separator-fallback / case-mismatch / …).
    pub message: Option<String>,
    /// Populated for Ambiguous (all candidate paths).
    pub candidates: Vec<String>,
}

impl ResolvedReference {
    pub(crate) fn not_found() -> Self {
        ResolvedReference {
            kind: ReferenceKind::NotFound,
            target_path: None,
            size: None,
            provenance: None,
            message: None,
            candidates: Vec::new(),
        }
    }
    /// Invariant: target_path is Some iff kind is a file/folder kind.
    pub(crate) fn debug_check_invariant(&self) {
        let has_path = matches!(
            self.kind,
            ReferenceKind::Image
                | ReferenceKind::Iframe
                | ReferenceKind::Pdf
                | ReferenceKind::Video
                | ReferenceKind::Audio
                | ReferenceKind::Model
                | ReferenceKind::FolderListing
                | ReferenceKind::FolderIndexIframe
                | ReferenceKind::Transclusion
                | ReferenceKind::Notebook
                | ReferenceKind::Table
        );
        debug_assert_eq!(
            has_path,
            self.target_path.is_some(),
            "target_path presence must match kind: {:?}",
            self.kind
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_has_no_path() {
        let r = ResolvedReference::not_found();
        assert_eq!(r.kind, ReferenceKind::NotFound);
        assert!(r.target_path.is_none());
        r.debug_check_invariant();
    }
}
