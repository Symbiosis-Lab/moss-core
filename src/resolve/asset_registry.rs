//! Single source of truth for all file types that moss can handle during drag, paste,
//! scan, and embed. Every consumer (ext_kind, embed_renderer, TS-generated registry)
//! derives from this table — no parallel lists.

use crate::resolve::ext_kind::ExtKind;

/// Metadata for a single file type supported by moss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetInfo {
    pub ext: &'static str,
    pub kind: ExtKind,
    pub mime: &'static str,
    pub can_embed: bool,      // renders inline in build/preview
    pub accept_on_drop: bool, // editor inserts an embed (vs import-only)
    pub embed_template: &'static str, // e.g. "![[{name}]]"
    pub label: &'static str,
    pub icon_key: &'static str,
}

const EMBED: &str = "![[{name}]]";

pub const ASSET_REGISTRY: &[AssetInfo] = &[
    // Markdown / transclusion
    AssetInfo { ext:"md",       kind:ExtKind::Transclusion, mime:"text/markdown", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Markdown", icon_key:"doc" },
    AssetInfo { ext:"markdown", kind:ExtKind::Transclusion, mime:"text/markdown", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Markdown", icon_key:"doc" },
    // Image (avif added — fixes the drop/build split)
    AssetInfo { ext:"png",  kind:ExtKind::Image, mime:"image/png",     can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"jpg",  kind:ExtKind::Image, mime:"image/jpeg",    can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"jpeg", kind:ExtKind::Image, mime:"image/jpeg",    can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"gif",  kind:ExtKind::Image, mime:"image/gif",     can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"svg",  kind:ExtKind::Image, mime:"image/svg+xml", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"webp", kind:ExtKind::Image, mime:"image/webp",    can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    AssetInfo { ext:"avif", kind:ExtKind::Image, mime:"image/avif",    can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Image", icon_key:"image" },
    // Image — viewer-only (NOT browser-embeddable; can_embed:false so no <img>/<picture> is emitted)
    AssetInfo { ext:"bmp",  kind:ExtKind::Image, mime:"image/bmp",     can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Image (not web-embeddable)", icon_key:"image" },
    AssetInfo { ext:"ico",  kind:ExtKind::Image, mime:"image/x-icon",  can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Image (not web-embeddable)", icon_key:"image" },
    AssetInfo { ext:"tiff", kind:ExtKind::Image, mime:"image/tiff",    can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Image (not web-embeddable)", icon_key:"image" },
    // Video — web-playable embed
    AssetInfo { ext:"mp4",  kind:ExtKind::Video, mime:"video/mp4",  can_embed:true,  accept_on_drop:true,  embed_template:EMBED, label:"Video", icon_key:"video" },
    AssetInfo { ext:"webm", kind:ExtKind::Video, mime:"video/webm", can_embed:true,  accept_on_drop:true,  embed_template:EMBED, label:"Video", icon_key:"video" },
    AssetInfo { ext:"mov",  kind:ExtKind::Video, mime:"video/quicktime", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Video", icon_key:"video" },
    AssetInfo { ext:"m4v",  kind:ExtKind::Video, mime:"video/x-m4v", can_embed:true, accept_on_drop:true,  embed_template:EMBED, label:"Video", icon_key:"video" },
    // Video — scanned/probed but NOT browser-embeddable (import-only; resolves the scan-vs-embed mismatch)
    AssetInfo { ext:"avi",  kind:ExtKind::Video, mime:"video/x-msvideo", can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Video (not web-playable)", icon_key:"video" },
    AssetInfo { ext:"mkv",  kind:ExtKind::Video, mime:"video/x-matroska", can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Video (not web-playable)", icon_key:"video" },
    // Audio
    AssetInfo { ext:"mp3",  kind:ExtKind::Audio, mime:"audio/mpeg", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"wav",  kind:ExtKind::Audio, mime:"audio/wav",  can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"ogg",  kind:ExtKind::Audio, mime:"audio/ogg",  can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"flac", kind:ExtKind::Audio, mime:"audio/flac", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"m4a",  kind:ExtKind::Audio, mime:"audio/mp4",  can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"opus", kind:ExtKind::Audio, mime:"audio/opus", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"aac",  kind:ExtKind::Audio, mime:"audio/aac",  can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Audio", icon_key:"audio" },
    AssetInfo { ext:"wma",  kind:ExtKind::Audio, mime:"audio/x-ms-wma", can_embed:false, accept_on_drop:false, embed_template:EMBED, label:"Audio (not web-playable)", icon_key:"audio" },
    // Notebook / iframe / pdf / model / table
    AssetInfo { ext:"ipynb", kind:ExtKind::Notebook, mime:"application/x-ipynb+json", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Notebook", icon_key:"notebook" },
    AssetInfo { ext:"html",  kind:ExtKind::Iframe, mime:"text/html", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Web page", icon_key:"web" },
    AssetInfo { ext:"htm",   kind:ExtKind::Iframe, mime:"text/html", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Web page", icon_key:"web" },
    AssetInfo { ext:"pdf",   kind:ExtKind::Pdf,   mime:"application/pdf", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"PDF", icon_key:"pdf" },
    AssetInfo { ext:"glb",   kind:ExtKind::Model, mime:"model/gltf-binary", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"3D model", icon_key:"model" },
    AssetInfo { ext:"gltf",  kind:ExtKind::Model, mime:"model/gltf+json",   can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"3D model", icon_key:"model" },
    AssetInfo { ext:"csv",   kind:ExtKind::Table, mime:"text/csv", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Table", icon_key:"table" },
    AssetInfo { ext:"tsv",   kind:ExtKind::Table, mime:"text/tab-separated-values", can_embed:true, accept_on_drop:true, embed_template:EMBED, label:"Table", icon_key:"table" },
];

/// Look up an asset by extension (case-insensitive, leading dot stripped).
/// Returns `None` if the extension is not in the registry.
pub fn asset_info(ext: &str) -> Option<&'static AssetInfo> {
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    ASSET_REGISTRY.iter().find(|a| a.ext == lower)
}

/// Return the full registry slice.
pub fn all_assets() -> &'static [AssetInfo] { ASSET_REGISTRY }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::ext_kind::ExtKind;

    #[test]
    fn image_avif_is_embeddable() {
        let a = asset_info("avif").expect("avif registered");
        assert_eq!(a.kind, ExtKind::Image);
        assert!(a.can_embed && a.accept_on_drop);
        assert_eq!(a.embed_template, "![[{name}]]");
    }

    #[test]
    fn avi_mkv_present_but_not_browser_embeddable() {
        for ext in ["avi", "mkv"] {
            let a = asset_info(ext).expect("registered");
            assert_eq!(a.kind, ExtKind::Video);
            assert!(!a.can_embed, "{ext} is not <video>-playable");
            assert!(!a.accept_on_drop, "{ext} imports only");
        }
    }

    #[test]
    fn every_renderer_extension_is_in_registry() {
        // Cross-checked against the renderer set in Task 1.2; here just assert core coverage.
        for ext in ["png","jpg","jpeg","gif","svg","webp","avif",
                    "mp4","webm","mov","m4v","mp3","wav","ogg","flac","m4a","opus","aac",
                    "ipynb","html","htm","pdf","glb","gltf","csv","tsv","md","markdown"] {
            assert!(asset_info(ext).is_some(), "{ext} missing from registry");
        }
    }

    #[test]
    fn bmp_ico_tiff_are_viewer_only_images() {
        // These were in the old IMAGE_EXTS set (→ 'image' category) but are NOT browser-embeddable.
        // They must be in the registry as Image kind so detectFileCategory still returns 'image',
        // but with can_embed:false + accept_on_drop:false so no <img>/<picture> is emitted in build.
        for ext in ["bmp", "ico", "tiff"] {
            let a = asset_info(ext).unwrap_or_else(|| panic!("{ext} missing from registry"));
            assert_eq!(a.kind, ExtKind::Image, "{ext} should be Image kind");
            assert!(!a.can_embed, "{ext} is not browser-embeddable");
            assert!(!a.accept_on_drop, "{ext} should not be inserted as embed on drop");
        }
    }
}
