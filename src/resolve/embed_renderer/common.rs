//! Shared helpers for typed embed renderers.
//!
//! These live here (not in each renderer) because Phase C/D renderers (pdf,
//! audio, video, 3d, etc.) all need to build `src` URLs, parse `|WxH` sizing
//! into HTML attrs, and escape attribute values. One copy, many callers.

use super::Sizing;

/// Build a `src` URL for embed elements: `path?query#fragment` (URL order,
/// independent of authoring order).
///
/// Retained for Phase 1's Stage 2 dispatcher, which may reconstruct embed
/// URLs from title-attribute params (`query=…`, `fragment=…`). Tests in this
/// module keep it exercised.
#[allow(dead_code)]
pub(super) fn build_src(
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> String {
    let mut out = String::from(path);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Parse `|WxH` into HTML `width="..."` and `height="..."` attribute strings
/// (each with a leading space). Empty strings if alias is missing or not a
/// sizing hint.
///
/// Phase 0 Stage 1 emitters now route sizing through title-attribute params
/// (`width=…` / `height=…`); this helper is retained for Phase 1's Stage 2
/// dispatcher to consume those params back into the HTML attribute form.
#[allow(dead_code)]
pub(super) fn dim_attrs(alias: Option<&str>) -> (String, String) {
    let Some(a) = alias else {
        return (String::new(), String::new());
    };
    match Sizing::parse(a) {
        Some(Sizing::Width(w)) => (format!(" width=\"{}\"", w.to_css()), String::new()),
        Some(Sizing::Box(w, h)) => (
            format!(" width=\"{}\"", w.to_css()),
            format!(" height=\"{}\"", h.to_css()),
        ),
        None => (String::new(), String::new()),
    }
}

/// Minimal HTML attribute-value escaper for `src`, `title`, and similar.
/// Escapes `& < > "`. Apostrophe is safe inside `"..."` attributes per HTML5.
///
/// Retained for Phase 1's Stage 2 dispatcher (which still emits HTML).
#[allow(dead_code)]
pub(super) fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build the ` data-width="value"` attribute fragment (with leading space)
/// for spec § P9 wrapper-width emission. Empty string when `None` so
/// formatters can splice unconditionally without an extra conditional.
///
/// Values reaching here are pre-validated by [`crate::media::match_width_token`]
/// (only `body | wide | page | screen`), so HTML escaping is a defensive
/// belt-and-braces and never actually substitutes.
///
/// Retained for Phase 1's Stage 2 dispatcher.
#[allow(dead_code)]
pub(super) fn width_attr(width: Option<&str>) -> String {
    match width {
        Some(w) => format!(r#" data-width="{}""#, html_escape_attr(w)),
        None => String::new(),
    }
}

/// Extract filename stem (no directory, no extension). Used by renderers that
/// want a human-readable label from a path.
pub(super) fn file_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem.to_string(),
        _ => filename.to_string(),
    }
}

/// Lowercase the filename extension (no leading dot). Returns empty string
/// if the path has no extension.
pub(super) fn path_extension_lower(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.rsplit_once('.') {
        Some((_stem, ext)) => ext.to_ascii_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_src_plain() {
        assert_eq!(build_src("file.html", None, None), "file.html");
    }

    #[test]
    fn test_build_src_with_query() {
        assert_eq!(
            build_src("file.html", Some("x=1&y=2"), None),
            "file.html?x=1&y=2"
        );
    }

    #[test]
    fn test_build_src_with_query_and_fragment() {
        assert_eq!(
            build_src("doc.html", Some("x=1"), Some("sec")),
            "doc.html?x=1#sec"
        );
    }

    #[test]
    fn test_html_escape_attr() {
        assert_eq!(html_escape_attr("a&b"), "a&amp;b");
        assert_eq!(html_escape_attr("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(html_escape_attr("say \"hi\""), "say &quot;hi&quot;");
    }

    #[test]
    fn test_file_stem() {
        assert_eq!(file_stem("photo.jpg"), "photo");
        assert_eq!(file_stem("dir/photo.jpg"), "photo");
        assert_eq!(file_stem("noext"), "noext");
        assert_eq!(file_stem(".dotfile"), ".dotfile");
    }

    #[test]
    fn test_path_extension_lower() {
        assert_eq!(path_extension_lower("photo.JPG"), "jpg");
        assert_eq!(path_extension_lower("dir/file.mp4"), "mp4");
        assert_eq!(path_extension_lower("noext"), "");
    }

    #[test]
    fn test_dim_attrs_none() {
        assert_eq!(dim_attrs(None), (String::new(), String::new()));
    }

    #[test]
    fn test_dim_attrs_width_only() {
        let (w, h) = dim_attrs(Some("400"));
        assert_eq!(w, " width=\"400px\"");
        assert_eq!(h, "");
    }

    #[test]
    fn test_dim_attrs_box() {
        let (w, h) = dim_attrs(Some("100%x600"));
        assert_eq!(w, " width=\"100%\"");
        assert_eq!(h, " height=\"600px\"");
    }
}
