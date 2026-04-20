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

/// Known language suffixes for home file detection.
///
/// This is a hardcoded list used only in moss-core (which has no access to
/// the i18n module). It covers the languages moss supports plus common extras.
///
/// Bare `"zh"` is accepted as shorthand for Simplified Chinese (`zh-hans`);
/// the normalization from `zh` → `zh-hans` happens in the i18n layer
/// (`Language::from_code`), not here. This list only governs which suffixes
/// are recognized as language suffixes at all.
const KNOWN_LANG_SUFFIXES: &[&str] = &[
    "en", "zh", "zh-hans", "zh-hant", "zh-cn", "zh-tw", "ja", "ko", "fr", "de", "es", "pt", "ru",
    "ar",
];

/// If `path` is rooted under a known language-tree directory (e.g.
/// `"zh-hans/about.md"`), return that directory component (e.g. `"zh-hans"`).
///
/// Returns `None` when the path has no parent directory (root-level file)
/// or when the first path component is not a recognized language code.
/// Matching is case-insensitive.
///
/// Used by wikilink resolution to prefer same-language-tree candidates.
///
/// ```
/// use moss_core::home::lang_tree_prefix;
/// assert_eq!(lang_tree_prefix("zh-hans/about.md"), Some("zh-hans"));
/// assert_eq!(lang_tree_prefix("ZH-HANS/about.md"), Some("ZH-HANS"));
/// assert_eq!(lang_tree_prefix("fr/deep/page.md"), Some("fr"));
/// assert_eq!(lang_tree_prefix("posts/hello.md"), None);
/// assert_eq!(lang_tree_prefix("index.md"), None);
/// assert_eq!(lang_tree_prefix(""), None);
/// ```
pub fn lang_tree_prefix(path: &str) -> Option<&str> {
    let first = path.split('/').next()?;
    // Must have at least one more component after it (so it's a directory).
    if first.len() == path.len() {
        return None;
    }
    if KNOWN_LANG_SUFFIXES.contains(&first.to_lowercase().as_str()) {
        Some(first)
    } else {
        None
    }
}

/// Strip a known language suffix from a file stem.
///
/// Returns the bare stem if the suffix after the last `.` is a recognized
/// language code, otherwise returns `None`.
///
/// ```
/// assert_eq!(moss_core::home::strip_lang_suffix("index.zh-hans"), Some("index"));
/// assert_eq!(moss_core::home::strip_lang_suffix("index.v2"), None);
/// assert_eq!(moss_core::home::strip_lang_suffix("index"), None);
/// ```
pub fn strip_lang_suffix(stem: &str) -> Option<&str> {
    if let Some(dot_pos) = stem.rfind('.') {
        let suffix = &stem[dot_pos + 1..];
        if KNOWN_LANG_SUFFIXES.contains(&suffix.to_lowercase().as_str()) {
            return Some(&stem[..dot_pos]);
        }
    }
    None
}

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

/// Check if a file stem acts as a home/index file in a given folder context.
///
/// Returns true if the stem is a recognized index stem (index, readme, etc.),
/// including language-suffixed variants like `index.zh-hans`.
/// Also matches self-named folder notes. Matching is case-insensitive.
pub fn is_home_file(stem: &str, parent_folder_name: &str) -> bool {
    // Direct index stem match
    if is_index_stem(stem) {
        return true;
    }

    // Language-suffixed index stem (e.g., "index.zh-hans")
    if let Some(bare) = strip_lang_suffix(stem) {
        if is_index_stem(bare) {
            return true;
        }
    }

    // Self-named folder note
    !parent_folder_name.is_empty() && stem.to_lowercase() == parent_folder_name.to_lowercase()
}

/// Find the home file among filenames, with folder-name awareness.
///
/// Like [`detect_home_file`], but also recognizes self-named folder notes
/// (e.g., `recipes.md` inside a folder named `recipes`). Priority order:
///
/// 1. INDEX_STEMS × .md (index.md > readme.md > _index.md > main.md)
/// 2. Language-suffixed index stems × .md (index.zh-hans.md > readme.en.md > ...)
/// 3. index.{pages,docx}
/// 4. Self-named: `foldername.md` (where foldername matches parent folder)
/// 5. First document alphabetically
pub fn detect_home_file_in_folder<'a>(
    filenames: &[&'a str],
    folder_name: &str,
) -> Option<&'a str> {
    // Priority 1: bare index stems × .md
    for stem in INDEX_STEMS {
        let target_md = format!("{}.md", stem);
        if let Some(&f) = filenames.iter().find(|f| f.to_lowercase() == target_md) {
            return Some(f);
        }
    }

    // Priority 2: language-suffixed index stems × .md
    // e.g., index.zh-hans.md, readme.en.md
    for stem in INDEX_STEMS {
        if let Some(&f) = filenames.iter().find(|f| {
            let lower = f.to_lowercase();
            if let Some(name_without_ext) = lower.strip_suffix(".md") {
                if let Some(bare) = strip_lang_suffix(name_without_ext) {
                    return bare == *stem;
                }
            }
            false
        }) {
            return Some(f);
        }
    }

    // Priority 3: index × non-markdown
    for ext in &["pages", "docx"] {
        let target = format!("index.{}", ext);
        if let Some(&f) = filenames.iter().find(|f| f.to_lowercase() == target) {
            return Some(f);
        }
    }

    // Priority 4: self-named folder note
    let self_named = format!("{}.md", folder_name.to_lowercase());
    if let Some(&f) = filenames.iter().find(|f| f.to_lowercase() == self_named) {
        return Some(f);
    }

    // Priority 5: first document alphabetically
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

/// Find the home file among a list of filenames in a single folder.
///
/// Convenience wrapper around [`detect_home_file_in_folder`] without
/// folder-name context. Self-named folder notes won't be recognized;
/// use `detect_home_file_in_folder` when the folder name is available.
pub fn detect_home_file<'a>(filenames: &[&'a str]) -> Option<&'a str> {
    detect_home_file_in_folder(filenames, "")
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

    // --- detect_home_file_in_folder ---

    #[test]
    fn test_detect_self_named_folder_note() {
        let files = vec!["recipes.md", "pasta.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "recipes"),
            Some("recipes.md")
        );
    }

    #[test]
    fn test_index_beats_self_named() {
        let files = vec!["recipes.md", "index.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "recipes"),
            Some("index.md")
        );
    }

    #[test]
    fn test_self_named_case_insensitive() {
        let files = vec!["Recipes.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "recipes"),
            Some("Recipes.md")
        );
    }

    #[test]
    fn test_readme_beats_self_named() {
        let files = vec!["recipes.md", "readme.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "recipes"),
            Some("readme.md")
        );
    }

    #[test]
    fn test_self_named_beats_alphabetical_fallback() {
        // Without folder context, "about.md" wins alphabetically.
        // With folder context, "recipes.md" wins as self-named.
        let files = vec!["about.md", "recipes.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "recipes"),
            Some("recipes.md")
        );
    }

    // --- is_home_file ---

    #[test]
    fn test_is_home_file_index_stem() {
        assert!(is_home_file("index", "anything"));
        assert!(is_home_file("readme", "anything"));
    }

    #[test]
    fn test_is_home_file_self_named() {
        assert!(is_home_file("recipes", "recipes"));
        assert!(is_home_file("刘果", "刘果"));
    }

    #[test]
    fn test_is_home_file_self_named_case_insensitive() {
        assert!(is_home_file("Recipes", "recipes"));
        assert!(is_home_file("recipes", "Recipes"));
    }

    #[test]
    fn test_is_home_file_no_match() {
        assert!(!is_home_file("about", "recipes"));
        assert!(!is_home_file("about", ""));
    }

    #[test]
    fn test_is_home_file_empty_folder_name() {
        // Empty folder name should not match anything except index stems
        assert!(is_home_file("index", ""));
        assert!(!is_home_file("about", ""));
    }

    #[test]
    fn test_in_folder_delegates_to_detect_home_file_for_index_stems() {
        // When index stems exist, folder name doesn't matter
        let files = vec!["index.md", "other.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "myfolder"),
            Some("index.md")
        );
    }

    // --- i18n: language-suffixed home files ---

    #[test]
    fn test_is_home_file_with_zh_hans_suffix() {
        assert!(is_home_file("index.zh-hans", "anything"));
    }

    #[test]
    fn test_is_home_file_with_en_suffix() {
        assert!(is_home_file("index.en", "anything"));
    }

    #[test]
    fn test_is_home_file_with_zh_hant_suffix() {
        assert!(is_home_file("readme.zh-hant", "anything"));
    }

    #[test]
    fn test_is_home_file_non_language_suffix_rejected() {
        // "v2" is not a language code — should NOT be treated as home file
        assert!(!is_home_file("index.v2", "anything"));
    }

    #[test]
    fn test_detect_home_bare_stem_wins_over_lang_suffix() {
        // When both bare index.md and index.zh-hans.md exist, bare wins
        let files = vec!["index.md", "index.zh-hans.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "root"),
            Some("index.md")
        );
    }

    #[test]
    fn test_detect_home_lang_suffix_recognized_when_no_bare() {
        // When only a language-suffixed index exists, it should still be home
        let files = vec!["index.zh-hans.md", "about.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "root"),
            Some("index.zh-hans.md")
        );
    }

    // --- Task 2.6: `zh` as shorthand for `zh-hans` ---

    #[test]
    fn test_strip_lang_suffix_zh_shorthand_accepted() {
        // `.zh` should be recognized as a language suffix (shorthand for zh-hans)
        assert_eq!(strip_lang_suffix("about.zh"), Some("about"));
    }

    #[test]
    fn test_strip_lang_suffix_zh_hans_still_works() {
        // Backward compat: zh-hans still works
        assert_eq!(strip_lang_suffix("about.zh-hans"), Some("about"));
    }

    #[test]
    fn test_strip_lang_suffix_zh_hant_still_works() {
        // zh-hant stays distinct (strip_lang_suffix only returns stem)
        assert_eq!(strip_lang_suffix("about.zh-hant"), Some("about"));
    }

    #[test]
    fn test_strip_lang_suffix_zh_tw_still_works() {
        // zh-tw stays distinct (strip_lang_suffix only returns stem)
        assert_eq!(strip_lang_suffix("about.zh-tw"), Some("about"));
    }

    #[test]
    fn test_is_home_file_with_zh_shorthand_suffix() {
        // `index.zh.md` stem is `index.zh` — should be recognized as home
        assert!(is_home_file("index.zh", "anything"));
    }

    #[test]
    fn test_detect_home_zh_shorthand_recognized() {
        // index.zh.md (shorthand) should be recognized as home when no bare exists
        let files = vec!["index.zh.md", "about.md"];
        assert_eq!(
            detect_home_file_in_folder(&files, "root"),
            Some("index.zh.md")
        );
    }
}
