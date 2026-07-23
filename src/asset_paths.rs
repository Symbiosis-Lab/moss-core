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

/// The raster source extensions that participate in the responsive ladder.
/// Single source of truth for the ladder-membership gate replicated across the
/// emission/registration/encode census sites — see [`ladder_rungs`]' census
/// doc. Phase B (Task 12) lifted webp's participation by editing ONLY this
/// predicate: the five pipeline sites (registration, `rungs_gated`,
/// `registered_rungs`, fingerprint-skip heal, `encode_rungs`) all derive rung
/// membership from here, so adding webp here made them include webp in lockstep.
///
/// NOTE ON `<picture>` vs `<img srcset>`: ladder membership is NOT the same as
/// "emits a `<picture><source>` webp CONVERSION". png/jpg/jpeg are re-encoded to
/// a differently-named `.webp` and emit `<picture><source srcset=X.webp>`; a
/// webp SOURCE is already webp, so [`to_webp`]`(src) == src` and it emits the
/// ladder directly on `<img srcset>` (no `<picture>` — a `<source>` identical to
/// the `<img>` is pointless). Callers that need the CONVERSION-only subset
/// (e.g. `should_skip`'s AlreadySmall carve-out, which must keep small webp
/// eligible to skip re-encode) combine this with [`is_webp_source_ext`]:
/// `is_ladder_source_ext(ext) && !is_webp_source_ext(ext)`.
///
/// Case-insensitive; `ext` is the extension WITHOUT the leading dot
/// ("png", "JPG", "jpeg", "webp"). Emission derives the gate from a full path
/// via `render/image.rs::is_raster_original` / `is_webp_source`; the pipeline
/// sites pass the scan-derived `item.ext` / a `source_file.extension()` string.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::is_ladder_source_ext;
/// assert!(is_ladder_source_ext("png"));
/// assert!(is_ladder_source_ext("JPG"));
/// assert!(is_ladder_source_ext("webp")); // joined the ladder in Phase B (Task 12)
/// ```
pub fn is_ladder_source_ext(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp")
}

/// True when `ext` is a WebP source extension (`webp`, case-insensitive).
///
/// A webp SOURCE is already webp: it does NOT get a differently-named
/// `<picture><source>` webp conversion — instead it carries the responsive
/// ladder directly on `<img srcset>` (Phase B, Task 12). Two callers need this
/// distinction that [`is_ladder_source_ext`] (which now unions webp in) can no
/// longer make alone:
/// - emission (`render/image.rs::synthesize_inner`) routes webp to the
///   `<img srcset>` branch instead of the `<picture>` branch;
/// - `should_skip`'s AlreadySmall carve-out excludes webp from the
///   "must-convert, never-skip" set so a small rung-free webp can still skip
///   the wasteful webp→webp re-encode.
///
/// `ext` is the extension WITHOUT the leading dot.
///
/// # Examples
/// ```
/// # use moss_core::asset_paths::is_webp_source_ext;
/// assert!(is_webp_source_ext("webp"));
/// assert!(is_webp_source_ext("WEBP"));
/// assert!(!is_webp_source_ext("png"));
/// assert!(!is_webp_source_ext("jpg"));
/// ```
pub fn is_webp_source_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("webp")
}

/// Max edge (px) of any deployed raster. Single source of truth — the encode
/// pipeline's `ImageCompressionConfig::default()` reads this constant, and the
/// srcset base-width descriptor caps at it. 2400 covers retina displays.
pub const DEPLOY_MAX_EDGE: u32 = 2400;

/// The responsive ladder: rung widths generated below the deployed base.
/// Must be strictly ascending — the `take_while` in [`ladder_rungs`] depends on it.
/// See docs/plans/2026-07-22-responsive-image-variants-design.md.
pub const LADDER: [u32; 2] = [800, 1600];

/// Width the deployed base variant actually has after the encoder's
/// aspect-preserving longest-EDGE resize (`img.resize(max_edge, max_edge,
/// Lanczos3)` in build/media/image.rs). When the longest edge exceeds
/// [`DEPLOY_MAX_EDGE`], BOTH dimensions shrink by the same ratio — for
/// portraits the deployed width is therefore SMALLER than `min(w, 2400)`:
/// a 3024×4032 portrait deploys at 1800×2400, so its base width is 1800.
///
/// Integer math (u64 multiply, truncating divide, floor at 1) mirrors the
/// image crate's `resize_dimensions` as closely as practical. srcset width
/// descriptors are browser HINTS: ±1px rounding drift vs the encoder's
/// float `.round()` is acceptable — a Task-5 cross-check test against real
/// encode output pins gross agreement.
pub fn deployed_width(natural_w: u32, natural_h: u32) -> u32 {
    let long_edge = natural_w.max(natural_h);
    if long_edge <= DEPLOY_MAX_EDGE {
        return natural_w;
    }
    ((natural_w as u64 * DEPLOY_MAX_EDGE as u64 / long_edge as u64) as u32).max(1)
}

/// Which ladder rungs exist for a source of `natural_w`×`natural_h` px.
///
/// DETERMINISTIC-AGREEMENT CONTRACT — the consuming-site census (keep this
/// list current; every site derives ladder membership from the same
/// scan-derived inputs):
///
/// 1. **emission** — the synthesizer (`render/image.rs::synthesize_inner`)
///    decides which rung srcset candidates appear in HTML;
/// 2. **registration** — blocking.rs's rung loop promises each rung URL via
///    `set_source_passthrough` + `set_pending`;
/// 3. **encode** — `encode_rungs` (build/media/image.rs) derives the same
///    ladder from oriented dims on the full-encode and warm-cache paths;
/// 4. **dispatch/sweep/Err-arm retraction** — the worker success/failure
///    arms' `registered_rungs` reconstruction (build/media/image.rs)
///    resolves or retracts every promise registration made;
/// 5. **fingerprint-skip heal** — the unchanged-fingerprint pass
///    (build/media/image.rs) rematerializes each rung it recomputes here.
///
/// A rung is emitted in HTML iff it is registered iff it is encoded. Never
/// add an input here that one of these sites cannot supply (e.g. encode
/// outcomes, cache state) — that is the parallel-oracle bug class deleted
/// 2026-05-20 (see build/media/image.rs:297-310).
///
/// EXIF-ORIENTATION CAVEAT (canonical; the pipeline sites back-reference here).
/// The "same scan-derived dims on every site" premise holds EXCEPT for an
/// EXIF-oriented (orientation 5-8, i.e. a 90°/270° rotation) png or webp. Scan
/// feeds emission/registration/sweep/heal `w`×`h` from
/// `extract_image_dimensions`, whose orientation read (`get_exif_orientation`,
/// scan.rs) is JPEG-GATED — for png/webp it returns the UNswapped header dims.
/// Encode's `decode_oriented`→`read_exif_orientation` is NOT gated: it swaps
/// dims for ANY container. So for an EXIF-rotated png/webp the encode's ladder
/// is computed from SWAPPED dims and can DIFFER from emission's (a
/// non-superset, not merely extra files): emission/registration can promise a
/// rung the encode never produces → registered-and-emitted-but-unencoded →
/// publish-404 (preview degrades to the placeholder via the unresolved-promise
/// sweep). This is rare (needs an EXIF-rotated png/webp), pre-existing for png,
/// and inherited by webp in Phase B. CODE FIX is deferred to a follow-up
/// (Task 14) — the fix direction is to ungate scan's orientation swap so scan
/// dims match `decode_oriented`; do NOT "fix" it by narrowing the encode side.
///
/// Rungs are strictly below the deployed base WIDTH (post-resize, see
/// [`deployed_width`] — portrait sources have a smaller base width than
/// `min(w, DEPLOY_MAX_EDGE)`) so the base descriptor never duplicates a
/// rung and no rung is ever wider than the base. Animated sources get no
/// ladder (animation-preserving multi-size re-encode is out of scope).
///
/// APNG: an animated PNG passes `is_raster_original` like any png, and the
/// BASE webp encode already FLATTENS it to a still today (`should_skip` in
/// build/media/image.rs sniffs animation only for gif/webp). Rungs run
/// through the same encode path and inherit the same flattening —
/// consistent by construction, no 404 risk.
///
/// ANIMATED-FLAG AGREEMENT (Phase B, Task 12). Only ONE of the five sites
/// passes a non-`false` flag: **emission** passes the scan-derived
/// `assets.is_animated(src)` for webp sources (an animated webp → empty
/// ladder → bare `<img>`, no srcset). The four pipeline sites (registration,
/// `rungs_gated`/`ladder_len`, `registered_rungs`, fingerprint-heal, plus
/// `encode_rungs`) keep the `false` literal, and that literal is PROVABLY
/// correct — not an assumption: an animated webp returns
/// `SkipReason::AnimatedWebp` in `should_skip`, so `collect_images_for_
/// conversion` drops it BEFORE it can become an `ImageConversionItem`. Every
/// webp that reaches the pipeline is therefore non-animated, and `false`
/// equals its real flag. Because scan's `sniff_is_animated` and `should_skip`
/// both call the SAME `is_animated_webp`, the emission verdict and the
/// pipeline's inclusion verdict never disagree: an animated webp is
/// simultaneously flagged in the snapshot (emission → no rungs) AND filtered
/// from the item list (pipeline → no rungs). Both sides produce zero rungs.
/// png/jpg/jpeg are never animated through this path (animated gif/webp are
/// not in the conversion set) and keep `false` everywhere too.
///
/// WARNING: any future pipeline-side skip that is NOT expressible as an
/// input to this function — e.g. copying the Y1 sized-raster APNG
/// verbatim-keep guard (build/media/image.rs ~line 830) onto rung encodes —
/// would create emitted-but-never-encoded rungs, i.e. the non-recoverable
/// chosen-`<source>` 404 (ADR-013). Task 5 must NOT copy that guard.
pub fn ladder_rungs(natural_w: u32, natural_h: u32, is_animated: bool) -> &'static [u32] {
    if is_animated {
        return &[];
    }
    let base = deployed_width(natural_w, natural_h);
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

    // ── is_ladder_source_ext tests ─────────────────────────────────

    #[test]
    fn is_ladder_source_ext_accepts_png_jpg_jpeg_webp_case_insensitively() {
        // Phase B (Task 12) added webp/WEBP: a webp SOURCE now participates in
        // the responsive ladder (emitted on `<img srcset>`, not `<picture>`).
        for ext in [
            "png", "PNG", "jpg", "JPG", "jpeg", "JPEG", "Jpg", "jPeG", "webp", "WEBP", "WebP",
        ] {
            assert!(is_ladder_source_ext(ext), "{ext} must be a ladder source");
        }
    }

    #[test]
    fn is_ladder_source_ext_rejects_non_ladder_formats() {
        // webp joined in Phase B (asserted above); these never join.
        for ext in ["gif", "svg", "avif", "heic", "bmp", "tiff", ""] {
            assert!(!is_ladder_source_ext(ext), "{ext} must NOT be a ladder source");
        }
    }

    #[test]
    fn is_webp_source_ext_matches_only_webp() {
        // The conversion-vs-webp split: webp is a ladder source that is NOT a
        // `<picture>` conversion (it emits `<img srcset>` and can skip a small
        // re-encode). png/jpg/jpeg are ladder sources that ARE conversions.
        for ext in ["webp", "WEBP", "WebP"] {
            assert!(is_webp_source_ext(ext), "{ext} must be a webp source");
        }
        for ext in ["png", "PNG", "jpg", "jpeg", "gif", "svg", ""] {
            assert!(!is_webp_source_ext(ext), "{ext} must NOT be a webp source");
        }
    }

    // ── ladder tests ───────────────────────────────────────────────

    #[test]
    fn ladder_is_strictly_ascending() {
        assert!(LADDER.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn ladder_rungs_below_deployed_base_only() {
        assert_eq!(ladder_rungs(2000, 1200, false), &[800, 1600][..]);
        assert_eq!(ladder_rungs(1601, 900, false), &[800, 1600][..]);
        // strict: no upscale, no dup of base
        assert_eq!(ladder_rungs(1600, 900, false), &[800][..]);
        assert_eq!(ladder_rungs(801, 600, false), &[800][..]);
        assert_eq!(ladder_rungs(800, 600, false), &[] as &[u32]);
        assert_eq!(ladder_rungs(0, 0, false), &[] as &[u32]);
    }

    #[test]
    fn ladder_rungs_capped_width_never_duplicates_base() {
        // 4000px-wide landscape deploys at DEPLOY_MAX_EDGE (2400) wide;
        // rungs must stay below the cap.
        assert_eq!(ladder_rungs(4000, 3000, false), &[800, 1600][..]);
        assert_eq!(ladder_rungs(2400, 1600, false), &[800, 1600][..]);
        // Square at exactly the cap: base 2400, both rungs below it.
        assert_eq!(ladder_rungs(2400, 2400, false), &[800, 1600][..]);
    }

    #[test]
    fn ladder_rungs_portrait_uses_post_resize_width() {
        // 3024×4032 portrait: the encoder shrinks the longest EDGE to 2400,
        // so the deployed base is 1800 wide — both rungs still below it.
        assert_eq!(ladder_rungs(3024, 4032, false), &[800, 1600][..]);
        // Extreme portrait 1179×8000: base width 353 — NO rung is below it,
        // so the ladder must be empty (a w800 rung would be WIDER than the
        // base: ladder inversion).
        assert_eq!(ladder_rungs(1179, 8000, false), &[] as &[u32]);
        // 1200×3600: base width exactly 800 — strict `<` excludes the 800 rung.
        assert_eq!(ladder_rungs(1200, 3600, false), &[] as &[u32]);
    }

    #[test]
    fn ladder_rungs_animated_is_empty() {
        assert_eq!(ladder_rungs(2000, 1200, true), &[] as &[u32]);
    }

    #[test]
    fn deployed_width_caps_longest_edge() {
        // Landscape: width IS the longest edge — capped directly.
        assert_eq!(deployed_width(4000, 3000), 2400);
        assert_eq!(deployed_width(2000, 1200), 2000);
        // Portrait: HEIGHT is the longest edge; width shrinks by the same
        // aspect-preserving ratio the encoder applies.
        assert_eq!(deployed_width(3024, 4032), 1800);
        assert_eq!(deployed_width(1179, 8000), 353);
        assert_eq!(deployed_width(1200, 3600), 800);
        // Square at the cap: untouched.
        assert_eq!(deployed_width(2400, 2400), 2400);
        // Degenerate sliver never collapses to 0.
        assert_eq!(deployed_width(1, 100_000), 1);
    }

    #[test]
    fn to_webp_rung_inserts_width_suffix() {
        assert_eq!(to_webp_rung("photo.jpg", 800), "photo.w800.webp");
        assert_eq!(to_webp_rung("a/b/photo.PNG", 1600), "a/b/photo.w1600.webp");
        assert_eq!(to_webp_rung("photo.webp", 800), "photo.w800.webp");
    }

    #[test]
    fn to_webp_rung_unknown_extension_fallback() {
        // Unknown extensions inherit to_webp's defensive-append behavior.
        assert_eq!(to_webp_rung("file.gif", 800), "file.gif.w800.webp");
    }
}
