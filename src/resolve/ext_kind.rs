//! Single source of truth mapping a file extension to its embed kind.
//! Pure; shared by build + editor. Replaces the duplication flagged at
//! `wikilink_dispatch.rs` (synth_kind_for_ext tables vs EmbedRenderer::extensions()).

/// The render family a non-folder, non-link target belongs to, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtKind {
    Image,
    Iframe,
    Pdf,
    Video,
    Audio,
    Model,
    Transclusion, // .md / .markdown
    Notebook,     // .ipynb
    Table,        // .csv / .tsv
    Other,        // unknown extension → caller treats as a Link
}

/// Classify a lowercase extension (no leading dot). Unknown → `Other`.
/// Derives from the asset registry SSOT so ext_kind and the registry never drift.
pub fn reference_kind_for_ext(ext: &str) -> ExtKind {
    crate::resolve::asset_registry::asset_info(ext)
        .map(|a| a.kind)
        .unwrap_or(ExtKind::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_each_family() {
        assert_eq!(reference_kind_for_ext("png"), ExtKind::Image);
        assert_eq!(reference_kind_for_ext("html"), ExtKind::Iframe);
        assert_eq!(reference_kind_for_ext("pdf"), ExtKind::Pdf);
        assert_eq!(reference_kind_for_ext("mp4"), ExtKind::Video);
        assert_eq!(reference_kind_for_ext("mp3"), ExtKind::Audio);
        assert_eq!(reference_kind_for_ext("glb"), ExtKind::Model);
        assert_eq!(reference_kind_for_ext("md"), ExtKind::Transclusion);
        assert_eq!(reference_kind_for_ext("ipynb"), ExtKind::Notebook);
        assert_eq!(reference_kind_for_ext("csv"), ExtKind::Table);
        assert_eq!(reference_kind_for_ext("xyz"), ExtKind::Other);
    }

    #[test]
    fn kind_matches_registry_for_all_known_exts() {
        use crate::resolve::asset_registry::all_assets;
        for a in all_assets() {
            assert_eq!(reference_kind_for_ext(a.ext), a.kind, "kind mismatch for {}", a.ext);
        }
        assert_eq!(reference_kind_for_ext("avif"), ExtKind::Image); // was Other before
    }
}
