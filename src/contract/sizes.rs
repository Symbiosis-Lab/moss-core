//! `sizes=` attribute values for responsive image emission, per render
//! context. Part of the theme-author surface: the values encode the DEFAULT
//! theme's layout (site.css). A theme that changes layout widths gets
//! slightly suboptimal (never broken) fetches; overrides are a future
//! contract extension, not built now (YAGNI).
//!
//! Layout facts these encode (verify against site.css when touching):
//! - content column: `--moss-content-width` = calc(42 * 1.125rem) = 47.25rem
//! - nav/content breakpoint: 48rem (see .claude/CLAUDE.md § "Navigation
//!   Responsive Breakpoints")
//! - `.moss-grid` runs 1–4 columns via data-columns within the content/wide column

/// Hero images and `data-width="wide|page|screen|full"` figures: span the viewport
/// (bounded by the 2400px deploy cap).
pub const SIZES_FULL_BLEED: &str = "100vw";

/// Default body figures/inline images: viewport-wide on small screens, the
/// content column (47.25rem) above the 48rem breakpoint.
pub const SIZES_BODY: &str = "(min-width: 48rem) 47.25rem, 100vw";

/// Folder-card covers and link-preview thumbs: grid cells, ~half column and up.
pub const SIZES_CARD: &str = "(min-width: 48rem) 24rem, 100vw";

/// Gallery thumbnails: 2–3 across on desktop.
pub const SIZES_GALLERY: &str = "(min-width: 48rem) 33vw, 100vw";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_strings_are_wellformed() {
        // Every constant must be non-empty and contain no double quotes
        // (they are interpolated into sizes="…").
        for s in [SIZES_FULL_BLEED, SIZES_BODY, SIZES_CARD, SIZES_GALLERY] {
            assert!(!s.is_empty());
            assert!(!s.contains('"'));
        }
    }
}
