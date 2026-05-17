//! Editor-facing shortcode scanner.
//!
//! Companion to `extract_shortcodes` (which returns typed AST nodes for the
//! build pipeline). `editor_scan` returns source-position information needed
//! by the CodeMirror plugin: opening-fence ranges, closing-fence ranges,
//! cell-divider ranges, and a flag indicating whether legacy `---` dividers
//! were used.
//!
//! Pure, no I/O. Safe to call from any Tauri thread.

use serde::{Deserialize, Serialize};

/// Half-open character offset range `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EditorRange {
    pub from: usize,
    pub to: usize,
}

/// One shortcode block as seen by the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EditorShortcodeBlock {
    /// Opening fence line (e.g. `:::grid 2`).
    pub open: EditorRange,
    /// Closing fence line (e.g. `:::`).
    pub close: EditorRange,
    /// Shortcode name (e.g. "grid", "buttons").
    pub name: String,
    /// Trailing args after the name (e.g. "2", "{.primary}").
    pub args: String,
    /// Top-level cell divider lines (only the dividers at this block's depth;
    /// nested-block dividers belong to their own block entry).
    pub dividers: Vec<EditorRange>,
}

/// Result of editor-side shortcode scanning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EditorScanResult {
    pub blocks: Vec<EditorShortcodeBlock>,
    /// True if any divider line in any grid block used the deprecated `---`
    /// form. The build pipeline emits a stronger warning; the editor uses
    /// this to surface a UI hint or console message.
    pub legacy_dash: bool,
}

/// Scan markdown for top-level shortcode blocks, returning source-position
/// information for the editor.
pub fn editor_scan(_markdown: &str) -> EditorScanResult {
    EditorScanResult {
        blocks: Vec::new(),
        legacy_dash: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_result() {
        let r = editor_scan("");
        assert!(r.blocks.is_empty());
        assert!(!r.legacy_dash);
    }
}
