//! Folder-listing embed: ![[/folder/|limit:N,sort:axis]]
//!
//! Pure-Rust path parsing + marker emission. The actual children
//! lookup + sort + HTML render happens in src-tauri (which has I/O).
//!
//! See docs/plans/2026-05-17-listing-sort-and-embeds-design.md.

use crate::resolve::embed_renderer::Sizing;
use crate::sort::SortAxis;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderEmbedParams {
    pub limit: Option<usize>,
    pub sort: Option<SortAxis>,
    pub style: Option<String>,   // "list" | "summary" | "grid"
    pub depth: Option<String>,   // "direct" | "all"
    pub group: Option<String>,   // "year" | "none"
    /// Raw sizing token (e.g. `"80%"`, `"800x600"`). Parsed to a `Sizing`
    /// at render time and applied ONLY to the static-index iframe branch
    /// (the card-grid listing branch ignores it). Stored raw so the
    /// pothole→marker→render round-trip stays a plain string.
    pub size: Option<String>,
    /// Internal: restrict children to this language-tree prefix (url_path prefix).
    /// Set by synthesize_children_marker for homepage default-mode; not user-facing.
    pub lang_tree: Option<String>,
    /// Internal: exclude folder pages that act as top-level nav items.
    /// Set by synthesize_children_marker for homepage default-mode; not user-facing.
    pub exclude_nav: bool,
}

/// Parse pipe-encoded params from the portion after `|`.
///
/// Format: `key:value,key:value` (e.g. `limit:5,sort:date`).
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
        } else if is_size_token(tok) {
            // A bare token that is unambiguously a sizing hint (ends in `%`,
            // `px`, `vh`, or is `<dim>x<dim>`) — but NOT a bare integer, which
            // stays a no-op bare flag so it never shadows `limit:N`. This is
            // the only place size enters the pothole grammar; all the keyed
            // params above carry a `:` and never reach this branch.
            out.size = Some(tok.to_string());
        }
        // unknown bare flags (e.g. legacy "more") silently ignored
    }
    out
}

/// Whether a bare pothole token is unambiguously a sizing hint.
///
/// True only when the token is NOT an all-ASCII-digit integer AND
/// `Sizing::parse` accepts it. The digit guard is what keeps a bare `5`
/// (which `Sizing::parse` would read as `5px`) from being mistaken for a
/// size — bare integers stay no-op flags, leaving `limit:N` the sole way
/// to set a limit.
fn is_size_token(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    let all_digits = tok.bytes().all(|b| b.is_ascii_digit());
    !all_digits && Sizing::parse(tok).is_some()
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
    if let Some(ref sz) = params.size {
        parts.push(format!("size={}", sz));
    }
    if let Some(n) = params.limit {
        parts.push(format!("limit={}", n));
    }
    if let Some(ref lt) = params.lang_tree {
        parts.push(format!("lang_tree={}", lt));
    }
    if params.exclude_nav {
        parts.push("exclude_nav".to_string());
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
    fn parses_more_flag_ignored() {
        // "more" is a legacy bare flag that is now silently ignored; limit still parses
        let p = parse_params("limit:5,more");
        assert_eq!(p.limit, Some(5));
    }

    #[test]
    fn parses_sort_override() {
        assert_eq!(parse_params("sort:date").sort, Some(SortAxis::Date));
    }

    #[test]
    fn parses_percent_size() {
        let p = parse_params("80%");
        assert_eq!(p.size, Some("80%".to_string()));
        assert_eq!(p.limit, None);
    }

    #[test]
    fn parses_box_size() {
        let p = parse_params("800x600");
        assert_eq!(p.size, Some("800x600".to_string()));
        assert_eq!(p.limit, None);
    }

    #[test]
    fn parses_vh_size() {
        assert_eq!(parse_params("80vh").size, Some("80vh".to_string()));
    }

    #[test]
    fn bare_px_token_is_not_recognized() {
        // `400px` is NOT a recognized size here because the shared
        // `Sizing::parse` splits on the literal `x` — `400px` → `("400p","")`
        // — and rejects both halves. This is a quirk of the shared parser
        // (the wikilink iframe dispatcher inherits the same gap), so a folder
        // embed `|400px` stays a no-op bare flag (unsized iframe), consistent
        // with `![[file.html|400px]]`. Plain `400` or `400%`/`80vh` work.
        assert_eq!(parse_params("400px").size, None);
    }

    #[test]
    fn bare_integer_is_not_size_and_not_limit() {
        // Collision guard: a bare integer must NOT become a size (it stays a
        // no-op bare flag), and it never set limit (only `limit:N` does).
        let p = parse_params("5");
        assert_eq!(p.size, None, "bare int must not be a size");
        assert_eq!(p.limit, None, "bare int must not set limit");
    }

    #[test]
    fn size_coexists_with_limit_key() {
        let p = parse_params("limit:3,80%");
        assert_eq!(p.limit, Some(3));
        assert_eq!(p.size, Some("80%".to_string()));
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
            sort: Some(SortAxis::Date),
            ..Default::default()
        };
        let m = emit_marker("/journal/", "index.md", &p);
        assert!(m.starts_with(MARKER_FOLDER_LIST));
        assert!(m.contains("path=/journal/"));
        assert!(m.contains("from=index.md"));
        assert!(m.contains("limit=3"));
        assert!(!m.contains("more"));
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
            size: Some("80%".to_string()),
            ..Default::default()
        };
        let m = emit_marker("/p/", "index.md", &p);
        assert!(m.contains("style=grid"));
        assert!(m.contains("depth=all"));
        assert!(m.contains("group=year"));
        assert!(m.contains("limit=5"));
        assert!(m.contains("size=80%"));
        assert!(!m.contains("more"));
    }

    #[test]
    fn marker_omits_size_when_absent() {
        let p = FolderEmbedParams::default();
        let m = emit_marker("/p/", "index.md", &p);
        assert!(!m.contains("size="));
    }
}
