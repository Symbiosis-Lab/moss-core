//! Folder-listing embed: ![[/folder/|limit:N,more,sort:axis]]
//!
//! Pure-Rust path parsing + marker emission. The actual children
//! lookup + sort + HTML render happens in src-tauri (which has I/O).
//!
//! See docs/plans/2026-05-17-listing-sort-and-embeds-design.md.

use crate::sort::SortAxis;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderEmbedParams {
    pub limit: Option<usize>,
    pub more: bool,
    pub sort: Option<SortAxis>,
    pub style: Option<String>,  // "list" | "summary" | "grid"
    pub depth: Option<String>,  // "direct" | "all"
    pub group: Option<String>,  // "year" | "none"
}

/// Parse pipe-encoded params from the portion after `|`.
///
/// Format: `key:value,key:value,flag` (e.g. `limit:5,more,sort:date`).
/// Unknown keys are silently ignored.
pub fn parse_params(raw: &str) -> FolderEmbedParams {
    let mut out = FolderEmbedParams::default();
    for tok in raw.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if let Some((k, v)) = tok.split_once(':') {
            match k.trim() {
                "limit" => out.limit = v.trim().parse().ok(),
                "sort" => {
                    out.sort = match v.trim() {
                        "date" => Some(SortAxis::Date),
                        "weight" => Some(SortAxis::Weight),
                        "title" => Some(SortAxis::Title),
                        _ => None,
                    }
                }
                "style" => out.style = Some(v.trim().to_string()),
                "depth" => out.depth = Some(v.trim().to_string()),
                "group" => out.group = Some(v.trim().to_string()),
                _ => {}
            }
        } else if tok == "more" {
            out.more = true;
        }
    }
    out
}

/// Marker prefix for folder-list embeds emitted by moss-core.
/// The src-tauri marker resolver (Task 16) reads everything between the prefix
/// and the terminator as `path=...|from=...|limit=N|more|sort=axis`. The `path`
/// is the user-written target (which may carry a leading `/`); `from` is the
/// source markdown file path, used for resolving relative paths against the
/// current document's location.
pub const MARKER_FOLDER_LIST: &str = "<!--MOSS_MARKER_FOLDER_LIST:";
pub const MARKER_END: &str = "-->";

pub fn emit_marker(path: &str, from: &str, params: &FolderEmbedParams) -> String {
    let mut parts = vec![format!("path={}", path), format!("from={}", from)];
    if let Some(ref s) = params.style {
        parts.push(format!("style={}", s));
    }
    if let Some(ref d) = params.depth {
        parts.push(format!("depth={}", d));
    }
    if let Some(ref g) = params.group {
        parts.push(format!("group={}", g));
    }
    if let Some(n) = params.limit {
        parts.push(format!("limit={}", n));
    }
    if params.more {
        parts.push("more".to_string());
    }
    if let Some(s) = params.sort {
        parts.push(format!(
            "sort={}",
            match s {
                SortAxis::Date => "date",
                SortAxis::Weight => "weight",
                SortAxis::Title => "title",
            }
        ));
    }
    format!("{}{}{}", MARKER_FOLDER_LIST, parts.join("|"), MARKER_END)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_limit() {
        assert_eq!(parse_params("limit:5").limit, Some(5));
    }

    #[test]
    fn parses_more_flag() {
        let p = parse_params("limit:5,more");
        assert_eq!(p.limit, Some(5));
        assert!(p.more);
    }

    #[test]
    fn parses_sort_override() {
        assert_eq!(parse_params("sort:date").sort, Some(SortAxis::Date));
    }

    #[test]
    fn empty_returns_defaults() {
        assert_eq!(parse_params(""), FolderEmbedParams::default());
    }

    #[test]
    fn unknown_keys_ignored() {
        let p = parse_params("limit:3,layout:minimal");
        assert_eq!(p.limit, Some(3));
        // layout: silently dropped — not implemented in v1
    }

    #[test]
    fn marker_roundtrips() {
        let p = FolderEmbedParams {
            limit: Some(3),
            more: true,
            sort: Some(SortAxis::Date),
            style: None,
            depth: None,
            group: None,
        };
        let m = emit_marker("/journal/", "index.md", &p);
        assert!(m.starts_with(MARKER_FOLDER_LIST));
        assert!(m.contains("path=/journal/"));
        assert!(m.contains("from=index.md"));
        assert!(m.contains("limit=3"));
        assert!(m.contains("more"));
        assert!(m.contains("sort=date"));
        assert!(m.ends_with(MARKER_END));
    }

    #[test]
    fn parses_style_grid() {
        assert_eq!(parse_params("style:grid").style, Some("grid".to_string()));
    }

    #[test]
    fn parses_depth_all() {
        assert_eq!(parse_params("depth:all").depth, Some("all".to_string()));
    }

    #[test]
    fn parses_group_year() {
        assert_eq!(parse_params("group:year").group, Some("year".to_string()));
    }

    #[test]
    fn marker_roundtrips_new_fields() {
        let p = FolderEmbedParams {
            style: Some("grid".to_string()),
            depth: Some("all".to_string()),
            group: Some("year".to_string()),
            limit: Some(5),
            more: false,
            sort: None,
        };
        let m = emit_marker("/p/", "index.md", &p);
        assert!(m.contains("style=grid"));
        assert!(m.contains("depth=all"));
        assert!(m.contains("group=year"));
        assert!(m.contains("limit=5"));
        assert!(!m.contains("more")); // not set
    }
}
