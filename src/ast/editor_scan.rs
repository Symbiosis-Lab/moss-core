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
    let mut legacy_dash = false;

    let mut current: Option<PartialBlock> = None;
    let mut depth: usize = 0;
    let mut current_is_grid: bool = false;

    let mut offset: usize = 0;
    for line in markdown.split_inclusive('\n') {
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
                current_is_grid = name == "grid";
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
        } else if match_open_fence(line_content).is_some() {
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
                current_is_grid = false;
            }
        } else if depth == 1 && current_is_grid {
            // Divider check only applies at depth 1 inside a grid block.
            if let Some(divider_range) =
                match_divider(line_content, line_start, &mut legacy_dash)
            {
                if let Some(c) = current.as_mut() {
                    c.dividers.push(divider_range);
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

/// Match a grid divider line. Recognizes exactly `+++` (canonical) and
/// exactly `---` (deprecated, sets `legacy_dash` to true). Both allow
/// surrounding whitespace but the line must contain nothing else.
///
/// Returns the source range covering the `+++` or `---` characters only,
/// excluding leading/trailing whitespace.
fn match_divider(
    line: &str,
    line_start: usize,
    legacy_dash: &mut bool,
) -> Option<EditorRange> {
    let trimmed = line.trim();
    let kind = match trimmed {
        "+++" => DividerKind::Canonical,
        "---" => DividerKind::LegacyDash,
        _ => return None,
    };

    let leading_ws = line.len() - line.trim_start().len();
    if matches!(kind, DividerKind::LegacyDash) {
        *legacy_dash = true;
    }
    Some(EditorRange {
        from: line_start + leading_ws,
        to: line_start + leading_ws + 3,
    })
}

enum DividerKind {
    Canonical,
    LegacyDash,
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

    #[test]
    fn grid_with_canonical_plus_divider() {
        let md = ":::grid 2\nleft\n+++\nright\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        let b = &r.blocks[0];
        assert_eq!(b.dividers.len(), 1);
        // "+++" starts after ":::grid 2\nleft\n" (10 + 5 = 15) and is 3 chars long.
        assert_eq!(b.dividers[0], EditorRange { from: 15, to: 18 });
        assert!(!r.legacy_dash);
    }

    #[test]
    fn grid_with_legacy_dash_divider_sets_flag() {
        let md = ":::grid 2\nleft\n---\nright\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.blocks[0].dividers.len(), 1);
        assert!(r.legacy_dash, "expected legacy_dash flag for --- divider");
    }

    #[test]
    fn dividers_only_at_top_depth() {
        // A nested :::buttons block contains a "---" line. That line is INSIDE
        // the nested block, so the outer grid should have zero dividers. (The
        // nested block isn't emitted at all per the nesting rule.)
        let md = ":::grid 2\n:::buttons\n---\n:::\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        assert!(r.blocks[0].dividers.is_empty());
        // legacy_dash is also false because the --- was inside a buttons block,
        // not a grid divider.
        assert!(!r.legacy_dash);
    }

    #[test]
    fn extra_plus_signs_are_not_divider() {
        // ++++ (four pluses) is NOT a divider — strict 3-char match.
        let md = ":::grid 2\nleft\n++++\nright\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        assert!(r.blocks[0].dividers.is_empty());
    }

    #[test]
    fn divider_with_leading_whitespace_is_recognized() {
        let md = ":::grid 2\nleft\n  +++\nright\n:::\n";
        let r = editor_scan(md);

        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.blocks[0].dividers.len(), 1);
        // Range covers the "+++" only, not the leading spaces.
        let div = r.blocks[0].dividers[0];
        let line_text = &md[div.from..div.to];
        assert_eq!(line_text, "+++");
    }
}
