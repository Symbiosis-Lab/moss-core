//! Pure output-path + output-relative-URL helpers shared by every asset/embed/
//! folder emitter. `resolve_path_with_overrides` slugifies intermediate dir
//! segments (generate_slug) and preserves the leaf; `reference_output_url`
//! computes the relative URL between two source paths IN OUTPUT SPACE so a
//! shared mixed-case prefix cancels (the Bug A fix, generalized).
//!
//! NOTE: this calls `crate::slug::generate_slug` (the text-to-slug primitive
//! page_map.rs's original used via the `super::slug::generate_slug` re-export),
//! NOT `crate::content_graph::generate_slug` (the path-to-key transform that
//! strips extensions). The two differ; `resolve_path_with_overrides` only ever
//! slugifies intermediate *directory* segments, never the leaf filename.

use crate::resolve::fuzzy_path::relative_asset_path;
use crate::slug::{generate_slug, normalize_separators};
use std::collections::HashMap;

pub fn resolve_path_with_overrides(path: &str, overrides: &HashMap<String, String>) -> String {
    // Normalize `\`→`/` so a Windows-authored source path slugs into the same
    // nested output as its `/`-form (otherwise the whole path is one leaf segment).
    let normalized = normalize_separators(path);
    let segments: Vec<&str> = normalized.split('/').collect();
    let last_idx = segments.len().saturating_sub(1);
    let mut resolved: Vec<String> = Vec::new();
    let mut cumulative = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            cumulative.push('/');
        }
        cumulative.push_str(seg);
        if let Some(override_slug) = overrides.get(&cumulative) {
            resolved.push(override_slug.clone());
        } else if i == last_idx {
            // Preserve filename / leaf-segment case so asset file references
            // (covers, video sources, image variants) keep matching the
            // file on disk.
            resolved.push((*seg).to_string());
        } else {
            // Slugify intermediate directory segments so source folder
            // casing/punctuation cannot leak into the URL or output path.
            resolved.push(generate_slug(seg));
        }
    }
    resolved.join("/")
}

/// Output-relative URL from `from_source`'s page to `target_source`, both
/// mapped through `resolve_path_with_overrides` so the result matches the
/// slugified output tree.
pub fn reference_output_url(
    from_source: &str,
    target_source: &str,
    overrides: &HashMap<String, String>,
) -> String {
    let from_out = resolve_path_with_overrides(from_source, overrides);
    let target_out = resolve_path_with_overrides(target_source, overrides);
    relative_asset_path(&from_out, &target_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_intermediate_preserves_leaf() {
        let o = HashMap::new();
        assert_eq!(
            resolve_path_with_overrides("Resources/cities-heat-map-app/index.html", &o),
            "resources/cities-heat-map-app/index.html"
        );
        assert_eq!(resolve_path_with_overrides("My App/index.html", &o), "my-app/index.html");
    }

    #[test]
    fn resolve_path_handles_backslash_separators() {
        let o = HashMap::new();
        // A backslash-separated source (Windows) must resolve to the same `/`-form
        // output — intermediate dir slugged, leaf case preserved — not collapse
        // into one segment (the asset-URL 404 bug).
        assert_eq!(
            resolve_path_with_overrides("Sub Dir\\Winter-Song.mov", &o),
            "sub-dir/Winter-Song.mov"
        );
        assert_eq!(
            resolve_path_with_overrides("Sub Dir\\Winter-Song.mov", &o),
            resolve_path_with_overrides("Sub Dir/Winter-Song.mov", &o),
        );
        assert!(!resolve_path_with_overrides("A\\B\\index.html", &o).contains('\\'));
    }

    #[test]
    fn output_url_cancels_shared_mixed_case_prefix() {
        let o = HashMap::new();
        assert_eq!(
            reference_output_url("Resources/index.md", "Resources/app/index.html", &o),
            "app/index.html"
        );
        assert_eq!(
            reference_output_url("Research.md", "Resources/app/index.html", &o),
            "resources/app/index.html"
        );
    }
}
