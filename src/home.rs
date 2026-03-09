//! Home file detection for the moss content model.
//!
//! Defines which filenames are recognized as a folder's "home" (index) page.
//! This is part of moss's content model — all generators and SSG plugins use
//! these conventions, ensuring consistent folder structure regardless of which
//! SSG backend is active.
//!
//! # Priority Order
//!
//! When multiple home file candidates exist in the same folder, the first
//! match in `INDEX_STEMS` wins:
//!
//! 1. `index` — standard convention
//! 2. `readme` — Git/GitHub convention
//! 3. `_index` — Hugo convention
//! 4. `main` — general fallback
//!
//! All matching is case-insensitive: `README.md`, `Index.md`, `MAIN.md` all match.

/// Recognized file stems (case-insensitive) that act as a folder's index page.
/// Priority order: first match wins when multiple candidates exist.
///
/// This is the single source of truth for home file detection across the entire
/// moss codebase. The Tauri layer, content graph, and SSG plugins all derive
/// their behavior from this constant.
pub const INDEX_STEMS: &[&str] = &["index", "readme", "_index", "main"];

/// Check if a filename stem (without extension) is a recognized home file.
///
/// Matching is case-insensitive.
///
/// ```
/// assert!(moss_core::home::is_index_stem("index"));
/// assert!(moss_core::home::is_index_stem("README"));
/// assert!(!moss_core::home::is_index_stem("about"));
/// ```
pub fn is_index_stem(stem: &str) -> bool {
    INDEX_STEMS.contains(&stem.to_lowercase().as_str())
}

/// Find the home file among a list of filenames in a single folder.
///
/// Takes bare filenames (not full paths) and returns the winning filename
/// based on `INDEX_STEMS` priority. Only considers `.md` files for non-`index`
/// stems; `index` also matches `.pages` and `.docx`.
///
/// Returns `None` if no home file candidate is found.
///
/// This is a pure function — no I/O. The caller is responsible for listing
/// files from the filesystem and passing them in.
pub fn detect_home_file<'a>(filenames: &[&'a str]) -> Option<&'a str> {
    // Priority 1: index stems × .md (first stem match wins)
    for stem in INDEX_STEMS {
        let target_md = format!("{}.md", stem);
        if let Some(&f) = filenames.iter().find(|f| f.to_lowercase() == target_md) {
            return Some(f);
        }
    }

    // Priority 2: index stem × non-markdown extensions
    for ext in &["pages", "docx"] {
        let target = format!("index.{}", ext);
        if let Some(&f) = filenames.iter().find(|f| f.to_lowercase() == target) {
            return Some(f);
        }
    }

    // Priority 3: first document file alphabetically
    let mut doc_files: Vec<&&str> = filenames
        .iter()
        .filter(|f| {
            let lower = f.to_lowercase();
            lower.ends_with(".md")
                || lower.ends_with(".pages")
                || lower.ends_with(".docx")
                || lower.ends_with(".doc")
        })
        .collect();
    doc_files.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    doc_files.first().map(|f| **f)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_index_stem ---

    #[test]
    fn test_is_index_stem_all_recognized() {
        assert!(is_index_stem("index"));
        assert!(is_index_stem("readme"));
        assert!(is_index_stem("_index"));
        assert!(is_index_stem("main"));
    }

    #[test]
    fn test_is_index_stem_case_insensitive() {
        assert!(is_index_stem("INDEX"));
        assert!(is_index_stem("README"));
        assert!(is_index_stem("Readme"));
        assert!(is_index_stem("MAIN"));
        assert!(is_index_stem("_Index"));
    }

    #[test]
    fn test_is_index_stem_rejects_non_stems() {
        assert!(!is_index_stem("about"));
        assert!(!is_index_stem("home"));
        assert!(!is_index_stem(""));
    }

    // --- detect_home_file ---

    #[test]
    fn test_detect_home_index_md_wins() {
        let files = vec!["README.md", "index.md", "main.md"];
        assert_eq!(detect_home_file(&files), Some("index.md"));
    }

    #[test]
    fn test_detect_home_readme_over_underscore_index() {
        let files = vec!["_index.md", "README.md"];
        assert_eq!(detect_home_file(&files), Some("README.md"));
    }

    #[test]
    fn test_detect_home_underscore_index_over_main() {
        let files = vec!["main.md", "_index.md"];
        assert_eq!(detect_home_file(&files), Some("_index.md"));
    }

    #[test]
    fn test_detect_home_case_insensitive() {
        let files = vec!["INDEX.MD"];
        // Should match even with uppercase
        assert_eq!(detect_home_file(&files), Some("INDEX.MD"));
    }

    #[test]
    fn test_detect_home_index_pages() {
        let files = vec!["index.pages"];
        assert_eq!(detect_home_file(&files), Some("index.pages"));
    }

    #[test]
    fn test_detect_home_md_over_pages() {
        let files = vec!["index.pages", "readme.md"];
        // readme.md (stem priority 2, .md) beats index.pages (non-md fallback)
        assert_eq!(detect_home_file(&files), Some("readme.md"));
    }

    #[test]
    fn test_detect_home_no_candidates() {
        let files = vec!["photo.jpg", "style.css"];
        assert_eq!(detect_home_file(&files), None);
    }

    #[test]
    fn test_detect_home_fallback_to_first_doc_alphabetically() {
        let files = vec!["zebra.md", "about.md"];
        assert_eq!(detect_home_file(&files), Some("about.md"));
    }

    #[test]
    fn test_detect_home_empty_list() {
        let files: Vec<&str> = vec![];
        assert_eq!(detect_home_file(&files), None);
    }
}
