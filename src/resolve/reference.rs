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

/// Classify a reference's inner text (target + optional |pothole / #anchor /
/// ?query) into a kind + resolved source path. Pure.
pub fn classify_reference(
    inner: &str,
    from_source: &str,
    ctx: &ReferenceContext,
) -> ResolvedReference {
    let inner = inner.trim();

    // External short-circuits (mirror classify_link's exception list).
    const EXTERNAL_PREFIXES: &[&str] =
        &["http://", "https://", "//", "mailto:", "tel:", "data:"];
    if EXTERNAL_PREFIXES.iter().any(|p| inner.starts_with(p)) {
        let mut r = ResolvedReference::not_found();
        r.kind = ReferenceKind::External { url: inner.to_string() };
        r.debug_check_invariant();
        return r;
    }
    // Pure anchor / query (no path component).
    if inner.starts_with('#') || inner.starts_with('?') {
        let mut r = ResolvedReference::not_found();
        r.kind = ReferenceKind::Anchor;
        return r;
    }

    // Split off |pothole, then #anchor.
    let (path_part, pothole) = match inner.split_once('|') {
        Some((p, rest)) => (p.trim(), Some(rest)),
        None => (inner, None),
    };
    let (path_no_anchor, anchor) = match path_part.split_once('#') {
        Some((p, a)) => (p.trim(), Some(a.to_string())),
        None => (path_part, None),
    };
    let size = pothole.and_then(crate::resolve::embed_renderer::Sizing::parse);

    use crate::resolve::asset_class::{resolve_asset_ref, AssetResolution};
    use crate::resolve::ext_kind::{reference_kind_for_ext, ExtKind};

    // (Folder arm is inserted here in Task 7, before the file arm.)

    // File arm.
    let ext = path_no_anchor.rsplit('.').next().unwrap_or("").to_lowercase();
    let ext_kind = reference_kind_for_ext(&ext);
    match resolve_asset_ref(path_no_anchor, from_source, ctx.assets) {
        AssetResolution::Resolved { root_rel, provenance } => {
            let kind = match ext_kind {
                ExtKind::Image => ReferenceKind::Image,
                ExtKind::Iframe => ReferenceKind::Iframe,
                ExtKind::Pdf => ReferenceKind::Pdf,
                ExtKind::Video => ReferenceKind::Video,
                ExtKind::Audio => ReferenceKind::Audio,
                ExtKind::Model => ReferenceKind::Model,
                ExtKind::Transclusion => ReferenceKind::Transclusion,
                ExtKind::Notebook => ReferenceKind::Notebook,
                ExtKind::Table => ReferenceKind::Table,
                // A resolved file with an UNKNOWN extension is a Link target (no embed path).
                ExtKind::Other => ReferenceKind::Link { anchor: anchor.clone() },
            };
            let is_link = matches!(kind, ReferenceKind::Link { .. });
            let mut r = ResolvedReference::not_found();
            r.kind = kind;
            r.target_path = if is_link { None } else { Some(root_rel) };
            r.size = size;
            r.provenance = Some(provenance);
            r.debug_check_invariant();
            r
        }
        AssetResolution::Ambiguous { candidates, .. } => {
            let mut r = ResolvedReference::not_found();
            r.kind = ReferenceKind::Ambiguous;
            r.candidates = candidates;
            r
        }
        AssetResolution::NotFound => ResolvedReference::not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::resolve::asset_class::FakeAssetIndex;
    use crate::resolve::folder_class::FakeFolderIndex;
    use crate::resolve::link_class::FakeUrlIndex;

    fn ctx<'a>(
        a: &'a FakeAssetIndex,
        f: &'a FakeFolderIndex,
        u: &'a FakeUrlIndex,
    ) -> ReferenceContext<'a> {
        ReferenceContext { assets: a, folders: f, urls: u }
    }

    #[test]
    fn external_url_is_external() {
        let a = FakeAssetIndex::new(&[]);
        let f = FakeFolderIndex::new();
        let u = FakeUrlIndex::new();
        let r = classify_reference("https://example.com/x", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::External { url: "https://example.com/x".into() });
        assert!(r.target_path.is_none());
    }

    #[test]
    fn bare_anchor_is_anchor() {
        let a = FakeAssetIndex::new(&[]);
        let f = FakeFolderIndex::new();
        let u = FakeUrlIndex::new();
        let r = classify_reference("#section", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::Anchor);
    }

    #[test]
    fn not_found_has_no_path() {
        let r = ResolvedReference::not_found();
        assert_eq!(r.kind, ReferenceKind::NotFound);
        assert!(r.target_path.is_none());
        r.debug_check_invariant();
    }

    #[test]
    fn image_file_resolves_to_image_kind() {
        let a = FakeAssetIndex::new(&["assets/photo.png"]);
        let f = FakeFolderIndex::new();
        let u = FakeUrlIndex::new();
        let r = classify_reference("photo.png", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::Image);
        assert_eq!(r.target_path.as_deref(), Some("assets/photo.png"));
        r.debug_check_invariant();
    }

    #[test]
    fn html_file_resolves_to_iframe_with_size() {
        let a = FakeAssetIndex::new(&["widgets/app.html"]);
        let f = FakeFolderIndex::new();
        let u = FakeUrlIndex::new();
        let r = classify_reference("widgets/app.html|800x600", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::Iframe);
        assert!(matches!(r.size, Some(crate::resolve::embed_renderer::Sizing::Box(_, _))));
    }

    #[test]
    fn ambiguous_file_match_sets_candidates() {
        let a = FakeAssetIndex::new(&["a/logo.png", "b/logo.png"]);
        let f = FakeFolderIndex::new();
        let u = FakeUrlIndex::new();
        let r = classify_reference("logo.png", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::Ambiguous);
        assert_eq!(r.candidates.len(), 2);
    }
}
