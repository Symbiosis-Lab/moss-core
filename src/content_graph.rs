//! In-memory index of content files, headings, and block IDs.
//!
//! `ContentGraph` is the read-only query structure built by `ContentGraphBuilder`.
//! It supports Obsidian-style fuzzy path resolution: exact path, filename-only,
//! folder notes, and ambiguity tiebreaking by longest common directory prefix.
//!
//! Pure Rust, zero I/O.

use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

// ---------------------------------------------------------------------------
// Path normalization helpers
// ---------------------------------------------------------------------------

/// NFC-normalize and lowercase a single path component.
fn normalize_component(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

/// NFC-normalize and lowercase every component of a `/`-separated path.
/// Also normalises backslashes to forward slashes and collapses runs of
/// separators.
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty())
        .map(normalize_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Extract the filename stem (no extension) from a normalized path.
fn filename_stem(normalized: &str) -> &str {
    let filename = normalized.rsplit('/').next().unwrap_or(normalized);
    match filename.rfind('.') {
        Some(pos) if pos > 0 => &filename[..pos],
        _ => filename,
    }
}

/// Extract the filename (with extension) from a path.
fn filename_with_ext(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Return the directory prefix components of a path as a Vec.
fn dir_components(path: &str) -> Vec<&str> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 1 {
        vec![]
    } else {
        parts[..parts.len() - 1].to_vec()
    }
}

/// Count the length of the longest common prefix between two component lists.
fn common_prefix_len(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

// ---------------------------------------------------------------------------
// Slug generation
// ---------------------------------------------------------------------------

/// Generate a URL slug from a relative file path.
///
/// Strips the file extension, normalizes separators to `/`, and lowercases.
/// Examples:
/// - `"posts/Hello World.md"` -> `"posts/hello-world"`
/// - `"guides/Setup.md"` -> `"guides/setup"`
/// - `"image.png"` -> `"image"`
/// - `"deep/path/to/file.txt"` -> `"deep/path/to/file"`
pub fn generate_slug(relative_path: &str) -> String {
    // Normalize separators
    let normalized = relative_path.replace('\\', "/");

    // Strip extension
    let without_ext = match normalized.rfind('.') {
        Some(dot_pos) => {
            // Only strip if the dot is in the filename part (after last /)
            let last_slash = normalized.rfind('/').unwrap_or(0);
            if dot_pos > last_slash {
                &normalized[..dot_pos]
            } else {
                &normalized
            }
        }
        None => &normalized,
    };

    // Lowercase and replace spaces with hyphens
    without_ext
        .to_lowercase()
        .replace(' ', "-")
}

// ---------------------------------------------------------------------------
// ContentGraph — the immutable, queryable index
// ---------------------------------------------------------------------------

/// An in-memory index of all content files, headings, and block IDs.
///
/// Created via [`ContentGraphBuilder::build`]. All lookups are
/// case-insensitive (NFC-normalized, lowercased).
#[derive(Debug, Clone)]
pub struct ContentGraph {
    /// All file paths (normalized), in insertion order.
    files: Vec<String>,

    /// Normalized filename stem (no extension, lowercase) -> list of file indices.
    filename_index: HashMap<String, Vec<usize>>,

    /// Normalized full path -> file index.
    path_index: HashMap<String, usize>,

    /// Normalized full path -> slug.
    slug_map: HashMap<String, String>,

    /// Normalized full path -> Vec<(heading_text, anchor_id)>.
    headings: HashMap<String, Vec<(String, String)>>,

    /// Normalized full path -> Vec<block_id>.
    blocks: HashMap<String, Vec<String>>,
}

impl ContentGraph {
    /// Resolve an Obsidian-style `reference` to a normalized path in this graph.
    ///
    /// Resolution chain (first match wins):
    /// 1. Exact normalized path
    /// 2. Exact + `.md`
    /// 3. Filename match (case-insensitive, without extension)
    /// 4. Filename + `.md` match
    /// 5. Folder note: `reference/index.md` or `reference/<reference>.md`
    ///
    /// When multiple candidates match (e.g. same filename in different dirs),
    /// the candidate sharing the longest common directory prefix with
    /// `from_path` wins.
    pub fn resolve_path(&self, reference: &str, from_path: &str) -> Option<String> {
        let norm_ref = normalize_path(reference);
        let norm_from = normalize_path(from_path);

        // 1. Exact path match
        if self.path_index.contains_key(&norm_ref) {
            return Some(self.files[self.path_index[&norm_ref]].clone());
        }

        // 2. Exact + .md
        let with_md = format!("{}.md", norm_ref);
        if self.path_index.contains_key(&with_md) {
            return Some(self.files[self.path_index[&with_md]].clone());
        }

        // 2b. Suffix match for partial paths (Obsidian shortest-path resolution).
        // e.g. "游记/index.md" matches "文字/游记/index.md"
        if norm_ref.contains('/') {
            let suffix = format!("/{}", norm_ref);
            let candidates: Vec<usize> = self.files.iter().enumerate()
                .filter(|(_, f)| normalize_path(f).ends_with(&suffix))
                .map(|(i, _)| i)
                .collect();
            if candidates.len() == 1 {
                return Some(self.files[candidates[0]].clone());
            }
            if candidates.len() > 1 {
                let from_dirs = dir_components(&norm_from);
                let best = candidates.iter().copied().max_by_key(|&idx| {
                    let candidate_dirs = dir_components(&self.files[idx]);
                    common_prefix_len(&candidate_dirs, &from_dirs)
                });
                if let Some(idx) = best {
                    return Some(self.files[idx].clone());
                }
            }
        }

        // 3/4. Filename match (stem, case-insensitive)
        // Skip stem matching when the reference is a multi-component path with an
        // index stem — falling back to just "index" would match every index.md in
        // the vault and return an arbitrary wrong result.
        let ref_stem = normalize_component(
            filename_stem(filename_with_ext(&norm_ref)),
        );
        let skip_stem = norm_ref.contains('/') && crate::home::is_index_stem(&ref_stem);
        if !skip_stem {
            if let Some(candidates) = self.filename_index.get(&ref_stem) {
                if candidates.len() == 1 {
                    return Some(self.files[candidates[0]].clone());
                }
                // Ambiguity tiebreaker: longest common directory prefix with from_path
                let from_dirs = dir_components(&norm_from);
                let best = candidates
                    .iter()
                    .copied()
                    .max_by_key(|&idx| {
                        let candidate_dirs = dir_components(&self.files[idx]);
                        common_prefix_len(&candidate_dirs, &from_dirs)
                    });
                if let Some(idx) = best {
                    return Some(self.files[idx].clone());
                }
            }
        }

        // 5. Folder note: try all recognized home file stems in priority order
        for stem in crate::home::INDEX_STEMS {
            let folder_index = format!("{}/{}.md", norm_ref, stem);
            if self.path_index.contains_key(&folder_index) {
                return Some(self.files[self.path_index[&folder_index]].clone());
            }
        }

        // 5b. Folder note: reference/<stem>.md  (self-named)
        let self_named = {
            let leaf = norm_ref.rsplit('/').next().unwrap_or(&norm_ref);
            format!("{}/{}.md", norm_ref, leaf)
        };
        if self.path_index.contains_key(&self_named) {
            return Some(self.files[self.path_index[&self_named]].clone());
        }

        None
    }

    /// Check whether the file at `path` has a heading with the given `anchor`.
    pub fn has_heading(&self, path: &str, anchor: &str) -> bool {
        let norm = normalize_path(path);
        let anchor_lower = normalize_component(anchor);
        self.headings
            .get(&norm)
            .map_or(false, |hs| hs.iter().any(|(_, a)| *a == anchor_lower))
    }

    /// Check whether the file at `path` has a block with the given `block_id`.
    pub fn has_block(&self, path: &str, block_id: &str) -> bool {
        let norm = normalize_path(path);
        let id_lower = normalize_component(block_id);
        self.blocks
            .get(&norm)
            .map_or(false, |bs| bs.iter().any(|b| *b == id_lower))
    }

    /// Return the slug for the given path, if registered.
    pub fn get_slug(&self, path: &str) -> Option<&str> {
        let norm = normalize_path(path);
        self.slug_map.get(&norm).map(|s| s.as_str())
    }

    /// All file paths in insertion order.
    pub fn all_files(&self) -> &[String] {
        &self.files
    }
}

// ---------------------------------------------------------------------------
// ContentGraphBuilder
// ---------------------------------------------------------------------------

/// Incrementally builds a [`ContentGraph`].
///
/// Call `add_file`, `add_headings`, `add_blocks` as content is scanned,
/// then `build()` to obtain the immutable graph.
#[derive(Debug, Default)]
pub struct ContentGraphBuilder {
    files: Vec<String>,
    filename_index: HashMap<String, Vec<usize>>,
    path_index: HashMap<String, usize>,
    slug_map: HashMap<String, String>,
    headings: HashMap<String, Vec<(String, String)>>,
    blocks: HashMap<String, Vec<String>>,
}

impl ContentGraphBuilder {
    /// Create a new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a content file.
    ///
    /// `relative_path` is the path relative to the source root (e.g.
    /// `"posts/hello.md"`). `slug` is the URL slug for this file.
    pub fn add_file(&mut self, relative_path: &str, slug: &str) {
        let norm = normalize_path(relative_path);

        // Skip duplicates: if this normalized path is already registered, don't
        // add another entry to `files` or `filename_index`.
        if self.path_index.contains_key(&norm) {
            return;
        }

        let idx = self.files.len();

        // Build filename stem index
        let stem = filename_stem(&norm).to_owned();
        self.filename_index.entry(stem).or_default().push(idx);

        // Build path index
        self.path_index.insert(norm.clone(), idx);

        // Slug map
        self.slug_map.insert(norm.clone(), slug.to_owned());

        // Store normalized path
        self.files.push(norm);
    }

    /// Register headings for a file. Each entry is `(heading_text, anchor_id)`.
    pub fn add_headings(&mut self, relative_path: &str, entries: Vec<(String, String)>) {
        let norm = normalize_path(relative_path);
        let normalized_entries = entries
            .into_iter()
            .map(|(text, anchor)| (text, normalize_component(&anchor)))
            .collect();
        self.headings.insert(norm, normalized_entries);
    }

    /// Register block IDs for a file.
    pub fn add_blocks(&mut self, relative_path: &str, ids: Vec<String>) {
        let norm = normalize_path(relative_path);
        let normalized_ids = ids.into_iter().map(|id| normalize_component(&id)).collect();
        self.blocks.insert(norm, normalized_ids);
    }

    /// Consume the builder and produce an immutable [`ContentGraph`].
    pub fn build(self) -> ContentGraph {
        ContentGraph {
            files: self.files,
            filename_index: self.filename_index,
            path_index: self.path_index,
            slug_map: self.slug_map,
            headings: self.headings,
            blocks: self.blocks,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Convenience: build a graph with common test files.
    fn sample_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("posts/hello.md", "/posts/hello");
        b.add_file("posts/world.md", "/posts/world");
        b.add_file("guides/hello.md", "/guides/hello");
        b.add_file("projects/index.md", "/projects");
        b.add_file("notes/daily/daily.md", "/notes/daily");
        b.add_headings(
            "posts/hello.md",
            vec![
                ("Introduction".into(), "introduction".into()),
                ("Getting Started".into(), "getting-started".into()),
            ],
        );
        b.add_blocks(
            "posts/hello.md",
            vec!["abc123".into(), "def456".into()],
        );
        b.build()
    }

    // 1. Basic file addition and resolution
    #[test]
    fn test_builder_adds_file() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("notes/first.md", "/notes/first");
        let g = b.build();

        assert_eq!(g.all_files(), &["notes/first.md"]);
        assert_eq!(
            g.resolve_path("notes/first.md", ""),
            Some("notes/first.md".into())
        );
    }

    // 2. Case-insensitive filename lookup
    #[test]
    fn test_filename_index_case_insensitive() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("Notes/MyFile.md", "/notes/myfile");
        let g = b.build();

        // Lookup with different casing
        assert_eq!(
            g.resolve_path("myfile", ""),
            Some("notes/myfile.md".into())
        );
        assert_eq!(
            g.resolve_path("MYFILE", ""),
            Some("notes/myfile.md".into())
        );
        assert_eq!(
            g.resolve_path("MyFile", ""),
            Some("notes/myfile.md".into())
        );
    }

    // 3. Lookup without .md extension
    #[test]
    fn test_filename_index_without_extension() {
        let g = sample_graph();

        // "world" (no extension) should find "posts/world.md"
        assert_eq!(
            g.resolve_path("world", ""),
            Some("posts/world.md".into())
        );
    }

    // 4. Ambiguous filename resolved by longest common directory prefix
    #[test]
    fn test_ambiguous_resolved_by_common_prefix() {
        let g = sample_graph();

        // "hello" is ambiguous: posts/hello.md vs guides/hello.md
        // from "posts/other.md" -> posts/hello.md should win
        assert_eq!(
            g.resolve_path("hello", "posts/other.md"),
            Some("posts/hello.md".into())
        );

        // from "guides/other.md" -> guides/hello.md should win
        assert_eq!(
            g.resolve_path("hello", "guides/other.md"),
            Some("guides/hello.md".into())
        );
    }

    // 5. Heading query
    #[test]
    fn test_headings_registered() {
        let g = sample_graph();

        assert!(g.has_heading("posts/hello.md", "introduction"));
        assert!(g.has_heading("posts/hello.md", "getting-started"));
        // Case-insensitive
        assert!(g.has_heading("posts/hello.md", "Introduction"));
        // Non-existent heading
        assert!(!g.has_heading("posts/hello.md", "nonexistent"));
        // Non-existent file
        assert!(!g.has_heading("nope.md", "introduction"));
    }

    // 6. Block ID query
    #[test]
    fn test_blocks_registered() {
        let g = sample_graph();

        assert!(g.has_block("posts/hello.md", "abc123"));
        assert!(g.has_block("posts/hello.md", "def456"));
        // Case-insensitive
        assert!(g.has_block("posts/hello.md", "ABC123"));
        // Non-existent block
        assert!(!g.has_block("posts/hello.md", "zzz"));
        // Non-existent file
        assert!(!g.has_block("nope.md", "abc123"));
    }

    // 7. Folder note resolution: [[projects]] -> projects/index.md
    #[test]
    fn test_folder_note_resolution() {
        let g = sample_graph();

        assert_eq!(
            g.resolve_path("projects", ""),
            Some("projects/index.md".into())
        );
    }

    // 7b. Self-named folder note: [[daily]] -> notes/daily/daily.md
    #[test]
    fn test_self_named_folder_note_resolution() {
        // "daily" as a filename stem appears in the filename index,
        // so it resolves via step 3 rather than step 5.
        let g = sample_graph();

        assert_eq!(
            g.resolve_path("daily", ""),
            Some("notes/daily/daily.md".into())
        );
    }

    // 7c. Self-named folder note via path
    #[test]
    fn test_self_named_folder_note_via_path() {
        let mut b = ContentGraphBuilder::new();
        // Only register the self-named note, no filename stem shortcut
        b.add_file("archive/archive.md", "/archive");
        let g = b.build();

        // Path-based reference should find it via the folder-note fallback
        assert_eq!(
            g.resolve_path("archive", ""),
            Some("archive/archive.md".into())
        );
    }

    // 8. Unresolved returns None
    #[test]
    fn test_unresolved_returns_none() {
        let g = sample_graph();

        assert_eq!(g.resolve_path("nonexistent", ""), None);
        assert_eq!(g.resolve_path("posts/missing.md", ""), None);
    }

    // 9. Exact relative path wins over filename
    #[test]
    fn test_exact_path_match() {
        let g = sample_graph();

        // Exact path should resolve directly, even though "hello" is ambiguous
        assert_eq!(
            g.resolve_path("guides/hello.md", "posts/other.md"),
            Some("guides/hello.md".into())
        );
    }

    // 10. Partial path match: "posts/hello" matches "posts/hello.md"
    #[test]
    fn test_partial_path_match() {
        let g = sample_graph();

        assert_eq!(
            g.resolve_path("posts/hello", ""),
            Some("posts/hello.md".into())
        );
        assert_eq!(
            g.resolve_path("posts/world", ""),
            Some("posts/world.md".into())
        );
    }

    // Slug lookup
    #[test]
    fn test_get_slug() {
        let g = sample_graph();

        assert_eq!(g.get_slug("posts/hello.md"), Some("/posts/hello"));
        assert_eq!(g.get_slug("Posts/Hello.md"), Some("/posts/hello"));
        assert_eq!(g.get_slug("nope.md"), None);
    }

    // all_files preserves insertion order
    #[test]
    fn test_all_files_order() {
        let g = sample_graph();

        assert_eq!(
            g.all_files(),
            &[
                "posts/hello.md",
                "posts/world.md",
                "guides/hello.md",
                "projects/index.md",
                "notes/daily/daily.md",
            ]
        );
    }

    // Unicode normalization (NFC)
    #[test]
    fn test_unicode_normalization() {
        let mut b = ContentGraphBuilder::new();
        // e + combining acute accent (NFD)
        b.add_file("caf\u{0065}\u{0301}.md", "/cafe");
        let g = b.build();

        // Lookup with NFC form (precomposed e-acute)
        assert_eq!(
            g.resolve_path("caf\u{00e9}.md", ""),
            Some("caf\u{00e9}.md".into())
        );
        // Lookup with NFD form
        assert_eq!(
            g.resolve_path("caf\u{0065}\u{0301}.md", ""),
            Some("caf\u{00e9}.md".into())
        );
    }

    // generate_slug tests
    #[test]
    fn test_generate_slug_strips_extension() {
        assert_eq!(generate_slug("posts/hello.md"), "posts/hello");
        assert_eq!(generate_slug("image.png"), "image");
    }

    #[test]
    fn test_generate_slug_lowercases() {
        assert_eq!(generate_slug("Posts/Hello.md"), "posts/hello");
    }

    #[test]
    fn test_generate_slug_replaces_spaces() {
        assert_eq!(generate_slug("posts/Hello World.md"), "posts/hello-world");
    }

    #[test]
    fn test_generate_slug_normalizes_backslashes() {
        assert_eq!(generate_slug("posts\\hello.md"), "posts/hello");
    }

    #[test]
    fn test_generate_slug_no_extension() {
        assert_eq!(generate_slug("readme"), "readme");
    }

    #[test]
    fn test_generate_slug_deep_path() {
        assert_eq!(
            generate_slug("deep/path/to/file.txt"),
            "deep/path/to/file"
        );
    }

    // When both index.md and self-named exist, filename stem match (step 3)
    // resolves "recipes" to recipes/recipes.md (unique stem match).
    // This is correct: the self-named note IS the folder's page in Obsidian links.
    #[test]
    fn test_resolve_self_named_via_filename_stem() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("recipes/index.md", "/recipes");
        b.add_file("recipes/recipes.md", "/recipes/recipes");
        let g = b.build();

        // "recipes" matches filename stem "recipes" → recipes/recipes.md (step 3)
        assert_eq!(
            g.resolve_path("recipes", "other.md"),
            Some("recipes/recipes.md".into())
        );
    }

    // When only index.md exists (no self-named), folder note fallback (step 5) works
    #[test]
    fn test_resolve_folder_note_fallback_to_index() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("recipes/index.md", "/recipes");
        b.add_file("recipes/pasta.md", "/recipes/pasta");
        let g = b.build();

        assert_eq!(
            g.resolve_path("recipes", "other.md"),
            Some("recipes/index.md".into())
        );
    }

    // Suffix match: partial path resolves when a deeper file ends with the reference
    #[test]
    fn test_suffix_match_partial_path() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("文字/游记/index.md", "/文字/游记");
        b.add_file("index.md", "/");
        let g = b.build();

        // "游记/index.md" doesn't exist at root, but "文字/游记/index.md" ends with it
        assert_eq!(
            g.resolve_path("游记/index.md", "index.md"),
            Some("文字/游记/index.md".into())
        );
    }

    // Suffix match with ambiguity uses from_path tiebreaker
    #[test]
    fn test_suffix_match_ambiguous_uses_tiebreaker() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("a/游记/index.md", "/a/游记");
        b.add_file("b/游记/index.md", "/b/游记");
        let g = b.build();

        // From "a/other.md", should prefer "a/游记/index.md"
        assert_eq!(
            g.resolve_path("游记/index.md", "a/other.md"),
            Some("a/游记/index.md".into())
        );
        // From "b/other.md", should prefer "b/游记/index.md"
        assert_eq!(
            g.resolve_path("游记/index.md", "b/other.md"),
            Some("b/游记/index.md".into())
        );
    }

    // Multi-component path with index stem should NOT fall back to arbitrary index.md
    #[test]
    fn test_multicomponent_index_path_no_false_match() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("交互实验/index.md", "/交互实验");
        b.add_file("文字/分布式信息网络/index.md", "/文字/分布式信息网络");
        let g = b.build();

        // "刘果/交互实验/index.md" has a wrong prefix — should return None,
        // not an arbitrary index.md like 文字/分布式信息网络/index.md
        assert_eq!(g.resolve_path("刘果/交互实验/index.md", ""), None);
    }
}
