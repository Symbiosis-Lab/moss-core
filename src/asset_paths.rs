//! Pure URL/path transform helpers — no filesystem access, no env lookups.
//! Used by moss-core's render functions (see crate::render::*) and by
//! upstream src-tauri call sites.
//!
//! # Design Intent
//!
//! Every component that derives an output path from a video or image source
//! path (e.g., .mov → .mp4, .mov → .thumb.jpg, .jpg → .webp) MUST use these
//! functions. This ensures consistency between:
//! - HTML attributes set by the synthesizers (src, data-placeholder-src, poster)
//! - AssetReady event paths emitted by the build pipeline
//! - AssetRegistry keys used by the preview server
//! - iframe-bridge.ts path comparisons
//!
//! If naming conventions change, they change here only.
//!
//! These are pure string functions — no I/O, no filesystem access.
//! They work on both root-relative paths ("videos/clip.mov") and
//! document-relative paths ("../clip.mov").

/// Known video extensions that get transcoded to .mp4.
const MOV_SUFFIXES: &[&str] = &[".mov", ".MOV", ".Mp4", ".MP4"];

/// All video extensions (including those that pass through unchanged for `to_mp4`
/// but still need thumbnail derivation).
const ALL_VIDEO_SUFFIXES: &[&str] = &[
    ".mov", ".MOV", ".mp4", ".MP4", ".Mp4", ".webm", ".WEBM",
];

/// Known image extensions that get re-encoded to .webp.
/// Order matters: longer suffixes (jpeg) before shorter (jpg) is not required
/// because we strip exact suffixes, but keep the list grouped by family for readability.
const IMAGE_SUFFIXES: &[&str] = &[
    ".jpg", ".JPG", ".jpeg", ".JPEG",
    ".png", ".PNG",
    ".webp", ".WEBP",
];

/// Converts a video source path to its .mp4 output path.
///
/// - `.mov` / `.MOV` → `.mp4`
/// - `.Mp4` / `.MP4` → `.mp4` (case normalization)
/// - `.mp4` → `.mp4` (unchanged)
/// - `.webm` → `.webm` (pass-through, not transcoded)
///
/// Any directory prefix is preserved.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::to_mp4;
/// assert_eq!(to_mp4("clip.mov"), "clip.mp4");
/// assert_eq!(to_mp4("../clip.MOV"), "../clip.mp4");
/// assert_eq!(to_mp4("clip.webm"), "clip.webm");
/// ```
pub fn to_mp4(source: &str) -> String {
    for suffix in MOV_SUFFIXES {
        if let Some(stem) = source.strip_suffix(suffix) {
            return format!("{stem}.mp4");
        }
    }
    // .mp4 (lowercase) and .webm pass through unchanged
    source.to_string()
}

/// Converts a video source path to its thumbnail path (.thumb.jpg).
///
/// All recognized video extensions are stripped and replaced with `.thumb.jpg`.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::to_thumb;
/// assert_eq!(to_thumb("clip.mov"), "clip.thumb.jpg");
/// assert_eq!(to_thumb("../clip.mp4"), "../clip.thumb.jpg");
/// ```
pub fn to_thumb(source: &str) -> String {
    for suffix in ALL_VIDEO_SUFFIXES {
        if let Some(stem) = source.strip_suffix(suffix) {
            return format!("{stem}.thumb.jpg");
        }
    }
    // Fallback: append .thumb.jpg (shouldn't happen with known video files)
    format!("{source}.thumb.jpg")
}

/// Like `to_thumb`, but returns `None` for non-video paths instead of
/// blindly appending `.thumb.jpg`.
///
/// Use this when the caller needs to *decide* whether a path is a video
/// (e.g. cover-color resolution), as opposed to forcing the conversion.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::to_thumb_if_video;
/// assert_eq!(to_thumb_if_video("clip.mov").as_deref(), Some("clip.thumb.jpg"));
/// assert_eq!(to_thumb_if_video("clip.MP4").as_deref(), Some("clip.thumb.jpg"));
/// assert_eq!(to_thumb_if_video("photo.jpg"), None);
/// ```
pub fn to_thumb_if_video(source: &str) -> Option<String> {
    for suffix in ALL_VIDEO_SUFFIXES {
        if let Some(stem) = source.strip_suffix(suffix) {
            return Some(format!("{stem}.thumb.jpg"));
        }
    }
    None
}

/// Converts an image source path to its WebP output path.
///
/// - `.jpg` / `.JPG` / `.jpeg` / `.JPEG` → `.webp`
/// - `.png` / `.PNG` → `.webp`
/// - `.webp` / `.WEBP` → `.webp` (case normalization only)
///
/// Any directory prefix is preserved. Unknown extensions fall back to
/// appending `.webp` (same defensive style as `to_thumb`).
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::to_webp;
/// assert_eq!(to_webp("photo.jpg"), "photo.webp");
/// assert_eq!(to_webp("../photo.PNG"), "../photo.webp");
/// assert_eq!(to_webp("photo.webp"), "photo.webp");
/// ```
pub fn to_webp(source: &str) -> String {
    for suffix in IMAGE_SUFFIXES {
        if let Some(stem) = source.strip_suffix(suffix) {
            return format!("{stem}.webp");
        }
    }
    // Fallback: append .webp (shouldn't happen with known image files)
    format!("{source}.webp")
}

/// Max edge (px) of any deployed raster. Single source of truth — the encode
/// pipeline's `ImageCompressionConfig::default()` reads this constant, and the
/// srcset base-width descriptor caps at it. 2400 covers retina displays.
pub const DEPLOY_MAX_EDGE: u32 = 2400;

/// The responsive ladder: rung widths generated below the deployed base.
/// See docs/plans/2026-07-22-responsive-image-variants-design.md.
pub const LADDER: [u32; 2] = [800, 1600];

/// Width the deployed base variant actually has (source width, capped).
pub fn deployed_width(natural_width: u32) -> u32 {
    natural_width.min(DEPLOY_MAX_EDGE)
}

/// Which ladder rungs exist for a source of `natural_width` px.
///
/// DETERMINISTIC-AGREEMENT CONTRACT: the registration loop (blocking.rs),
/// the encode worker (build/media/image.rs), and the synthesizer
/// (render/image.rs) all call this with the same scan-derived inputs. A rung
/// is emitted in HTML iff it is registered iff it is encoded. Never add an
/// input here that one of the three sides cannot supply (e.g. encode
/// outcomes, cache state) — that is the parallel-oracle bug class deleted
/// 2026-05-20 (see build/media/image.rs:297-310).
///
/// Rungs are strictly below the deployed base width so the base descriptor
/// never duplicates a rung. Animated sources get no ladder (animation-
/// preserving multi-size re-encode is out of scope).
pub fn ladder_rungs(natural_width: u32, is_animated: bool) -> &'static [u32] {
    if is_animated {
        return &[];
    }
    let base = deployed_width(natural_width);
    let n = LADDER.iter().take_while(|&&w| w < base).count();
    &LADDER[..n]
}

/// Rung variant URL: `photo.jpg` + 800 → `photo.w800.webp`.
/// Same derivation style as [`to_webp`]; preserves directory prefix.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::to_webp_rung;
/// assert_eq!(to_webp_rung("photo.jpg", 800), "photo.w800.webp");
/// assert_eq!(to_webp_rung("../photo.PNG", 1600), "../photo.w1600.webp");
/// ```
pub fn to_webp_rung(source: &str, width: u32) -> String {
    let webp = to_webp(source);
    let stem = webp.strip_suffix(".webp").unwrap_or(&webp);
    format!("{stem}.w{width}.webp")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── to_mp4 tests ───────────────────────────────────────────────

    #[test]
    fn test_to_mp4_mov() {
        assert_eq!(to_mp4("clip.mov"), "clip.mp4");
    }

    #[test]
    fn test_to_mp4_mov_uppercase() {
        assert_eq!(to_mp4("clip.MOV"), "clip.mp4");
    }

    #[test]
    fn test_to_mp4_mp4_mixed_case() {
        assert_eq!(to_mp4("clip.Mp4"), "clip.mp4");
    }

    #[test]
    fn test_to_mp4_mp4_uppercase() {
        assert_eq!(to_mp4("clip.MP4"), "clip.mp4");
    }

    #[test]
    fn test_to_mp4_already_mp4() {
        assert_eq!(to_mp4("clip.mp4"), "clip.mp4");
    }

    #[test]
    fn test_to_mp4_webm_passthrough() {
        assert_eq!(to_mp4("clip.webm"), "clip.webm");
    }

    #[test]
    fn test_to_mp4_with_relative_prefix() {
        assert_eq!(to_mp4("../clip.mov"), "../clip.mp4");
    }

    #[test]
    fn test_to_mp4_with_dot_prefix() {
        assert_eq!(to_mp4("./clip.MOV"), "./clip.mp4");
    }

    #[test]
    fn test_to_mp4_with_directory() {
        assert_eq!(to_mp4("videos/clip.mov"), "videos/clip.mp4");
    }

    #[test]
    fn test_to_mp4_unicode_filename() {
        assert_eq!(to_mp4("videos/冬日之歌.mov"), "videos/冬日之歌.mp4");
    }

    #[test]
    fn test_to_mp4_space_in_name() {
        assert_eq!(to_mp4("./Morning Mist.mov"), "./Morning Mist.mp4");
    }

    // ── to_thumb tests ─────────────────────────────────────────────

    #[test]
    fn test_to_thumb_mov() {
        assert_eq!(to_thumb("clip.mov"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_mp4() {
        assert_eq!(to_thumb("clip.mp4"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_mov_uppercase() {
        assert_eq!(to_thumb("clip.MOV"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_mp4_uppercase() {
        assert_eq!(to_thumb("clip.MP4"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_mp4_mixed() {
        assert_eq!(to_thumb("clip.Mp4"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_webm() {
        assert_eq!(to_thumb("clip.webm"), "clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_with_directory() {
        assert_eq!(to_thumb("videos/clip.mov"), "videos/clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_with_relative_prefix() {
        assert_eq!(to_thumb("../clip.mp4"), "../clip.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_unicode() {
        assert_eq!(to_thumb("videos/冬日之歌.mov"), "videos/冬日之歌.thumb.jpg");
    }

    #[test]
    fn test_to_thumb_space() {
        assert_eq!(to_thumb("./Morning Mist.MOV"), "./Morning Mist.thumb.jpg");
    }

    // ── to_thumb_if_video tests ────────────────────────────────────

    #[test]
    fn test_to_thumb_if_video_recognizes_lowercase_extensions() {
        assert_eq!(to_thumb_if_video("clip.mov").as_deref(), Some("clip.thumb.jpg"));
        assert_eq!(to_thumb_if_video("clip.mp4").as_deref(), Some("clip.thumb.jpg"));
        assert_eq!(to_thumb_if_video("clip.webm").as_deref(), Some("clip.thumb.jpg"));
    }

    #[test]
    fn test_to_thumb_if_video_recognizes_uppercase_extensions() {
        // Regression for the original bug: uppercase video extensions
        // (.MOV from iPhone, .MP4 from some cameras) must be detected.
        assert_eq!(to_thumb_if_video("clip.MOV").as_deref(), Some("clip.thumb.jpg"));
        assert_eq!(to_thumb_if_video("clip.MP4").as_deref(), Some("clip.thumb.jpg"));
        assert_eq!(to_thumb_if_video("clip.Mp4").as_deref(), Some("clip.thumb.jpg"));
        assert_eq!(to_thumb_if_video("clip.WEBM").as_deref(), Some("clip.thumb.jpg"));
    }

    #[test]
    fn test_to_thumb_if_video_with_directory() {
        assert_eq!(to_thumb_if_video("音乐/cover.mp4").as_deref(), Some("音乐/cover.thumb.jpg"));
        assert_eq!(to_thumb_if_video("../clip.MOV").as_deref(), Some("../clip.thumb.jpg"));
    }

    #[test]
    fn test_to_thumb_if_video_returns_none_for_images() {
        assert_eq!(to_thumb_if_video("photo.jpg"), None);
        assert_eq!(to_thumb_if_video("photo.png"), None);
        assert_eq!(to_thumb_if_video("photo.webp"), None);
        assert_eq!(to_thumb_if_video("photo.JPEG"), None);
    }

    #[test]
    fn test_to_thumb_if_video_returns_none_for_unknown() {
        // Unlike `to_thumb`, no `.thumb.jpg` fallback is appended.
        // Callers using this helper want a definitive yes/no.
        assert_eq!(to_thumb_if_video("file.gif"), None);
        assert_eq!(to_thumb_if_video("file"), None);
        assert_eq!(to_thumb_if_video(""), None);
    }

    // ── to_webp tests ──────────────────────────────────────────────

    #[test]
    fn test_to_webp_jpg() {
        assert_eq!(to_webp("photo.jpg"), "photo.webp");
    }

    #[test]
    fn test_to_webp_jpg_uppercase() {
        assert_eq!(to_webp("photo.JPG"), "photo.webp");
    }

    #[test]
    fn test_to_webp_jpeg() {
        assert_eq!(to_webp("photo.jpeg"), "photo.webp");
    }

    #[test]
    fn test_to_webp_jpeg_uppercase() {
        assert_eq!(to_webp("photo.JPEG"), "photo.webp");
    }

    #[test]
    fn test_to_webp_png() {
        assert_eq!(to_webp("photo.png"), "photo.webp");
    }

    #[test]
    fn test_to_webp_png_uppercase() {
        assert_eq!(to_webp("photo.PNG"), "photo.webp");
    }

    #[test]
    fn test_to_webp_already_webp() {
        assert_eq!(to_webp("photo.webp"), "photo.webp");
    }

    #[test]
    fn test_to_webp_webp_uppercase() {
        assert_eq!(to_webp("photo.WEBP"), "photo.webp");
    }

    #[test]
    fn test_to_webp_with_directory() {
        assert_eq!(to_webp("images/photo.jpg"), "images/photo.webp");
    }

    #[test]
    fn test_to_webp_with_relative_prefix() {
        assert_eq!(to_webp("../photo.png"), "../photo.webp");
    }

    #[test]
    fn test_to_webp_with_dot_prefix() {
        assert_eq!(to_webp("./photo.JPG"), "./photo.webp");
    }

    #[test]
    fn test_to_webp_unicode_filename() {
        assert_eq!(to_webp("images/冬日之歌.jpg"), "images/冬日之歌.webp");
    }

    #[test]
    fn test_to_webp_space_in_name() {
        assert_eq!(to_webp("./Morning Mist.png"), "./Morning Mist.webp");
    }

    #[test]
    fn test_to_webp_unknown_extension_fallback() {
        // Unknown extensions fall back to appending .webp (matches to_thumb's defensive style).
        assert_eq!(to_webp("file.gif"), "file.gif.webp");
    }

    // ── ladder tests ───────────────────────────────────────────────

    #[test]
    fn ladder_rungs_below_natural_width_only() {
        assert_eq!(ladder_rungs(2000, false), &[800, 1600][..]);
        assert_eq!(ladder_rungs(1601, false), &[800, 1600][..]);
        assert_eq!(ladder_rungs(1600, false), &[800][..]);   // strict: no upscale, no dup of base
        assert_eq!(ladder_rungs(801, false), &[800][..]);
        assert_eq!(ladder_rungs(800, false), &[] as &[u32]);
        assert_eq!(ladder_rungs(0, false), &[] as &[u32]);
    }

    #[test]
    fn ladder_rungs_capped_width_never_duplicates_base() {
        // 4000px source deploys at DEPLOY_MAX_EDGE (2400); rungs must stay below the cap.
        assert_eq!(ladder_rungs(4000, false), &[800, 1600][..]);
        assert_eq!(ladder_rungs(2400, false), &[800, 1600][..]);
    }

    #[test]
    fn ladder_rungs_animated_is_empty() {
        assert_eq!(ladder_rungs(2000, true), &[] as &[u32]);
    }

    #[test]
    fn deployed_width_caps_at_max_edge() {
        assert_eq!(deployed_width(4000), 2400);
        assert_eq!(deployed_width(2000), 2000);
    }

    #[test]
    fn to_webp_rung_inserts_width_suffix() {
        assert_eq!(to_webp_rung("photo.jpg", 800), "photo.w800.webp");
        assert_eq!(to_webp_rung("a/b/photo.PNG", 1600), "a/b/photo.w1600.webp");
        assert_eq!(to_webp_rung("photo.webp", 800), "photo.w800.webp");
    }
}
