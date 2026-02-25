//! Fuzzy path resolution and relative URL computation.
//!
//! Wraps [`ContentGraph::resolve_path`] with typed results and provides
//! [`relative_url`] for generating correct relative links between files
//! using pretty URL format (directory-based).

use crate::content_graph::ContentGraph;

use super::parent_dir;

/// The result of resolving a reference against the content graph.
#[derive(Debug, PartialEq, Clone)]
pub enum ResolvedRef {
    /// The reference resolved to a file at this normalized path.
    Found(String),
    /// The reference could not be resolved to any known file.
    Unresolved,
}

/// Resolve a reference string against the content graph.
///
/// This is a thin wrapper over [`ContentGraph::resolve_path`] that returns
/// a typed [`ResolvedRef`] instead of `Option<String>`.
///
/// # Arguments
///
/// * `reference` — the link target (e.g. `"hello"`, `"posts/hello.md"`)
/// * `graph` — the content graph to search
/// * `from_path` — the file containing the link (for disambiguation)
pub fn resolve_reference(reference: &str, graph: &ContentGraph, from_path: &str) -> ResolvedRef {
    match graph.resolve_path(reference, from_path) {
        Some(path) => ResolvedRef::Found(path),
        None => ResolvedRef::Unresolved,
    }
}

/// Compute the relative URL from one file to another using pretty URL format.
///
/// Both paths should be relative to the source root (e.g. `"posts/hello.md"`).
/// The output uses directory-based pretty URLs:
///
/// - `"posts/hello.md"` becomes the URL `"posts/hello/"` (a directory)
/// - `"posts/index.md"` becomes the URL `"posts/"` (the directory itself)
/// - The returned URL is relative from `from_path`'s directory
///
/// # Examples
///
/// ```
/// use moss_core::resolve::fuzzy_path::relative_url;
///
/// // Same directory
/// assert_eq!(relative_url("posts/a.md", "posts/b.md"), "b/");
///
/// // Parent directory
/// assert_eq!(relative_url("posts/deep/a.md", "posts/b.md"), "../b/");
///
/// // Target is index.md (URL is the directory)
/// assert_eq!(relative_url("posts/a.md", "posts/index.md"), "./");
/// ```
pub fn relative_url(from_path: &str, to_path: &str) -> String {
    let from_dir = parent_dir(from_path);
    let to_url_path = to_pretty_url_dir(to_path);

    // Split both into components
    let from_parts: Vec<&str> = if from_dir.is_empty() {
        vec![]
    } else {
        from_dir.split('/').collect()
    };

    let to_parts: Vec<&str> = if to_url_path.is_empty() {
        vec![]
    } else {
        // Remove trailing slash for splitting, then we'll add it back
        let trimmed = to_url_path.trim_end_matches('/');
        if trimmed.is_empty() {
            vec![]
        } else {
            trimmed.split('/').collect()
        }
    };

    // Find common prefix length
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Number of ".." needed to go up from from_dir
    let ups = from_parts.len() - common;

    // Remaining path components after the common prefix
    let remaining = &to_parts[common..];

    let mut result = String::new();

    if ups == 0 && remaining.is_empty() {
        // Same directory — from_dir IS the target URL directory
        return "./".to_string();
    }

    // Add "../" for each level we need to go up
    for _ in 0..ups {
        result.push_str("../");
    }

    // Add the remaining path components
    for (i, part) in remaining.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(part);
    }

    // Ensure trailing slash for pretty URLs
    if !result.ends_with('/') {
        result.push('/');
    }

    result
}

/// Convert a source file path to its pretty URL directory path.
///
/// - `"posts/hello.md"` -> `"posts/hello"`  (the URL becomes `posts/hello/`)
/// - `"posts/index.md"` -> `"posts"`        (the URL becomes `posts/`)
/// - `"index.md"`       -> `""`             (the URL becomes `/`)
fn to_pretty_url_dir(path: &str) -> String {
    // Strip the file extension
    let without_ext = match path.rfind('.') {
        Some(pos) => &path[..pos],
        None => path,
    };

    // If the filename is "index", the URL is the parent directory
    let filename = without_ext.rsplit('/').next().unwrap_or(without_ext);
    if filename == "index" {
        let parent = parent_dir(without_ext);
        parent.to_string()
    } else {
        without_ext.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    /// Build a graph with common test files.
    fn sample_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("posts/hello.md", "/posts/hello");
        b.add_file("posts/world.md", "/posts/world");
        b.add_file("guides/hello.md", "/guides/hello");
        b.add_file("projects/index.md", "/projects");
        b.add_file("about.md", "/about");
        b.add_file("images/photo.png", "/images/photo.png");
        b.add_file("index.md", "/");
        b.build()
    }

    // -- resolve_reference tests --

    #[test]
    fn test_resolve_exact_relative() {
        let graph = sample_graph();
        assert_eq!(
            resolve_reference("posts/hello.md", &graph, "posts/world.md"),
            ResolvedRef::Found("posts/hello.md".into())
        );
    }

    #[test]
    fn test_resolve_filename_only() {
        let graph = sample_graph();
        // "world" is unique, so filename-only lookup should find it
        assert_eq!(
            resolve_reference("world", &graph, ""),
            ResolvedRef::Found("posts/world.md".into())
        );
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let graph = sample_graph();
        // "World" with different casing should still resolve
        assert_eq!(
            resolve_reference("World", &graph, ""),
            ResolvedRef::Found("posts/world.md".into())
        );
        assert_eq!(
            resolve_reference("ABOUT", &graph, ""),
            ResolvedRef::Found("about.md".into())
        );
    }

    #[test]
    fn test_resolve_unresolved() {
        let graph = sample_graph();
        assert_eq!(
            resolve_reference("nonexistent", &graph, "posts/hello.md"),
            ResolvedRef::Unresolved
        );
        assert_eq!(
            resolve_reference("missing/page.md", &graph, ""),
            ResolvedRef::Unresolved
        );
    }

    #[test]
    fn test_resolve_image_reference() {
        let graph = sample_graph();
        // Non-markdown file (image) should also resolve
        assert_eq!(
            resolve_reference("images/photo.png", &graph, "posts/hello.md"),
            ResolvedRef::Found("images/photo.png".into())
        );
    }

    // -- relative_url tests --

    #[test]
    fn test_relative_url_same_dir() {
        // "posts/a.md" to "posts/b.md" → "b/"
        assert_eq!(relative_url("posts/a.md", "posts/b.md"), "b/");
    }

    #[test]
    fn test_relative_url_parent() {
        // "posts/deep/a.md" to "posts/b.md" → "../b/"
        assert_eq!(relative_url("posts/deep/a.md", "posts/b.md"), "../b/");
    }

    #[test]
    fn test_relative_url_sibling_dir() {
        // "blog/a.md" to "notes/b.md" → "../notes/b/"
        assert_eq!(relative_url("blog/a.md", "notes/b.md"), "../notes/b/");
    }

    #[test]
    fn test_relative_url_index() {
        // URL to index.md is the directory itself
        // "posts/a.md" to "posts/index.md" → "./"
        assert_eq!(relative_url("posts/a.md", "posts/index.md"), "./");

        // "blog/a.md" to "posts/index.md" → "../posts/"
        assert_eq!(relative_url("blog/a.md", "posts/index.md"), "../posts/");

        // root index: "posts/a.md" to "index.md" → "../"
        assert_eq!(relative_url("posts/a.md", "index.md"), "../");
    }

    #[test]
    fn test_relative_url_root_to_nested() {
        // "index.md" to "posts/hello.md" → "posts/hello/"
        assert_eq!(relative_url("index.md", "posts/hello.md"), "posts/hello/");
    }

    #[test]
    fn test_relative_url_nested_to_root() {
        // "posts/hello.md" to "about.md" → "../about/"
        assert_eq!(relative_url("posts/hello.md", "about.md"), "../about/");
    }
}
