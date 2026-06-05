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

    // Folder arm: trailing slash, or the target resolves to a directory.
    let looks_like_folder = path_no_anchor.ends_with('/');
    let folder_rel: Option<String> = if let Some(abs) = path_no_anchor.strip_prefix('/') {
        Some(abs.trim_end_matches('/').to_string())
    } else if looks_like_folder {
        // source-relative lexical join against from_source's directory
        let from_dir = crate::resolve::parent_dir(from_source);
        let mut parts: Vec<&str> = if from_dir.is_empty() {
            vec![]
        } else {
            from_dir.split('/').collect()
        };
        for seg in path_no_anchor.trim_end_matches('/').split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        Some(parts.join("/"))
    } else {
        None
    };
    if let Some(folder_rel) = folder_rel {
        // Only treat this as a folder reference when it is one: an explicit
        // trailing slash, or a path that the folder index resolves to a real
        // directory. A leading-slash path WITHOUT a trailing slash (e.g. an
        // absolute file embed `/assets/photo.png`) is NOT a folder — it must
        // fall through to the file arm and resolve as the asset it names.
        let is_folder = looks_like_folder || ctx.folders.is_dir(&folder_rel);
        if is_folder {
            if ctx.folders.dir_has_markdown_index(&folder_rel) {
                let mut r = ResolvedReference::not_found();
                r.kind = ReferenceKind::FolderListing;
                r.target_path = Some(folder_rel);
                r.size = size;
                r.debug_check_invariant();
                return r;
            }
            if ctx.folders.dir_has_static_index(&folder_rel).is_some() {
                let mut r = ResolvedReference::not_found();
                r.kind = ReferenceKind::FolderIndexIframe;
                r.target_path = Some(folder_rel);
                r.size = size;
                r.debug_check_invariant();
                return r;
            }
            // A confirmed folder (explicit trailing slash, or a real directory)
            // without an index is NotFound — it must NOT fall through to the
            // file arm (a folder path is never a file asset).
            return ResolvedReference::not_found();
        }
    }

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

    #[test]
    fn folder_with_static_index_is_iframe() {
        let a = FakeAssetIndex::new(&[]);
        let mut f = FakeFolderIndex::new();
        f.dirs.insert("Resources/app".into());
        f.static_index.insert("Resources/app".into(), "index.html".into());
        let u = FakeUrlIndex::new();
        let r = classify_reference("/Resources/app/", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::FolderIndexIframe);
        assert_eq!(r.target_path.as_deref(), Some("Resources/app"));
        r.debug_check_invariant();
    }

    #[test]
    fn folder_with_markdown_index_is_listing() {
        let a = FakeAssetIndex::new(&[]);
        let mut f = FakeFolderIndex::new();
        f.dirs.insert("news".into());
        f.md_index.insert("news".into());
        let u = FakeUrlIndex::new();
        let r = classify_reference("/news/", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::FolderListing);
        r.debug_check_invariant();
    }

    #[test]
    fn absolute_file_embed_resolves_to_image() {
        // A leading-slash path with NO trailing slash, naming a real asset, is a
        // file embed — not a folder. The folder arm must let it fall through to
        // the file arm so `![[/assets/photo.png]]` resolves as an Image.
        let a = FakeAssetIndex::new(&["assets/photo.png"]);
        let f = FakeFolderIndex::new(); // NOT a dir, no indexes
        let u = FakeUrlIndex::new();
        let r = classify_reference("/assets/photo.png", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::Image);
        assert_eq!(r.target_path.as_deref(), Some("assets/photo.png"));
        r.debug_check_invariant();
    }

    #[test]
    fn trailing_slash_unresolved_folder_is_not_found() {
        let a = FakeAssetIndex::new(&[]);
        let f = FakeFolderIndex::new(); // empty: not a dir, no indexes
        let u = FakeUrlIndex::new();
        let r = classify_reference("/ghost/", "page.md", &ctx(&a, &f, &u));
        assert_eq!(r.kind, ReferenceKind::NotFound);
    }
}
