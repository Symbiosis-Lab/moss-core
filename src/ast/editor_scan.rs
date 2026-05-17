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
pub fn editor_scan(markdown: &str) -> EditorScanResult {
    let mut blocks = Vec::new();
    let legacy_dash = false;

    // State for the currently open top-level block. We only track top-level
    // blocks; nested blocks (depth > 1) are skipped (their bodies belong to
    // the parent's "body" from the editor's perspective).
    let mut current: Option<PartialBlock> = None;
    let mut depth: usize = 0;

    // Walk by lines while tracking byte offsets.
    let mut offset: usize = 0;
    for line in markdown.split_inclusive('\n') {
        // `split_inclusive` keeps the trailing '\n'. `line_end` is the offset
        // just past the line content (before the '\n' if present).
        let has_newline = line.ends_with('\n');
        let line_len_without_newline = if has_newline {
            line.len() - 1
        } else {
            line.len()
        };
        let line_content = &line[..line_len_without_newline];
        let line_start = offset;
        let line_end = offset + line_len_without_newline;

        if depth == 0 {
            if let Some((name, args)) = match_open_fence(line_content) {
                current = Some(PartialBlock {
                    open: EditorRange {
                        from: line_start,
                        to: line_end,
                    },
                    name: name.to_string(),
                    args: args.to_string(),
                    dividers: Vec::new(),
                });
                depth = 1;
            }
        } else {
            if match_open_fence(line_content).is_some() {
                depth += 1;
            } else if is_close_fence(line_content) {
                depth -= 1;
                if depth == 0 {
                    if let Some(partial) = current.take() {
                        blocks.push(EditorShortcodeBlock {
                            open: partial.open,
                            close: EditorRange {
                                from: line_start,
                                to: line_end,
                            },
                            name: partial.name,
                            args: partial.args,
                            dividers: partial.dividers,
                        });
                    }
                }
            }
        }

        offset += line.len();
    }

    EditorScanResult {
        blocks,
        legacy_dash,
    }
}

struct PartialBlock {
    open: EditorRange,
    name: String,
    args: String,
    dividers: Vec<EditorRange>,
}

/// Match `:::name args...` opening fence. Returns `(name, args)` if matched.
/// Mirrors the `SHORTCODE_OPEN` regex previously in `cm-shortcode.ts` but
/// without depending on the regex crate.
fn match_open_fence(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let leading_ws = line.len() - trimmed.len();
    let rest = trimmed.strip_prefix(":::")?;

    // The name starts immediately after `:::` (no space).
    let name_start_in_line = leading_ws + 3;
    let name_bytes = rest
        .bytes()
        .take_while(|b| {
            b.is_ascii_alphanumeric() || *b == b'_'
        })
        .count();
    if name_bytes == 0 {
        return None;
    }
    let name = &line[name_start_in_line..name_start_in_line + name_bytes];

    let after_name = &line[name_start_in_line + name_bytes..];
    let args = after_name.trim();
    Some((name, args))
}

/// Match `:::` closing fence (no name).
fn is_close_fence(line: &str) -> bool {
    line.trim() == ":::"
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

    #[test]
    fn finds_single_grid_block_no_dividers() {
        let md = ":::grid 2\nleft\nright\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        let b = &r.blocks[0];
        assert_eq!(b.name, "grid");
        assert_eq!(b.args, "2");
        assert_eq!(b.open, EditorRange { from: 0, to: 9 });   // ":::grid 2"
        assert_eq!(b.close, EditorRange { from: 21, to: 24 }); // ":::"
        assert!(b.dividers.is_empty());
        assert!(!r.legacy_dash);
    }

    #[test]
    fn nested_blocks_only_emit_outer() {
        // Outer :::grid contains a nested :::buttons. We only emit the outer
        // block; the inner one's open/close fences don't escape.
        let md = ":::grid 2\n:::buttons\n[a](#)\n:::\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.blocks[0].name, "grid");
    }

    #[test]
    fn unclosed_block_is_dropped() {
        let md = ":::grid 2\nleft\nright\n";
        let r = editor_scan(md);
        assert!(r.blocks.is_empty());
    }

    #[test]
    fn two_sibling_blocks() {
        let md = ":::buttons\n[a](#)\n:::\n\n:::gallery\n[]()\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 2);
        assert_eq!(r.blocks[0].name, "buttons");
        assert_eq!(r.blocks[1].name, "gallery");
    }
}
