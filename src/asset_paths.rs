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
}
