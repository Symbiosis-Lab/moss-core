//! Forward and backward link/embed edges between pages (moss#922 Stage 4).
//!
//! `DepGraph` answers "who links to / embeds this page?" — the question
//! Stage 5's facade-gated render skip needs to widen a changed page's
//! minimal render set to its backlinks. It is a standalone type, not a
//! `ContentGraph` field: `ContentGraph` is a structural path index built
//! from file paths alone (zero content reads, see `content_graph.rs`)
//! while `DepGraph` is built from parsed per-page edges — same
//! type-vs-populator split as `ContentGraph` itself (the type here; the
//! `Vec<ParsedDocument>` → edges glue lives in `src-tauri`, which is not a
//! moss-core dependency).
//!
//! Rebuilt fresh every build from scratch — cheap (a handful of `HashMap`
//! inserts over ~one page count), same argument as why `ContentGraph`
//! itself is never persisted. See the "Don't persist ContentGraph; don't
//! use ObjectStore" section of
//! `docs/archive/2026-07-31-incremental-build-facade-diff.md`.

use crate::resolve::{LinkType, OutgoingLink};
use std::collections::HashMap;

/// Directed link/embed edges between pages, keyed by source path.
///
/// Built once per build via [`DepGraph::build`]; queried via [`DepGraph::backlinks`]
/// and [`DepGraph::back_embeds`]. `Embed` edges are a subset already present
/// in `forward_links`/`backlinks` — `forward_embeds`/`back_embeds` narrow to
/// just `LinkType::Embed` because embeds are more render-relevant than plain
/// links (a transcluded page's body IS part of the embedding page's output).
#[derive(Debug, Clone, Default)]
pub struct DepGraph {
    forward_links: HashMap<String, Vec<String>>,
    forward_embeds: HashMap<String, Vec<String>>,
    backlinks: HashMap<String, Vec<String>>,
    back_embeds: HashMap<String, Vec<String>>,
}

impl DepGraph {
    /// Build a `DepGraph` from each page's source path and the outgoing
    /// links it resolved during parsing. Order of `pages` does not affect
    /// the result — edge lists within a bucket follow input order for
    /// determinism, but no page's presence depends on any other's.
    pub fn build<'a, I>(pages: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a [OutgoingLink])>,
    {
        let mut graph = DepGraph::default();
        for (source_path, outgoing) in pages {
            for link in outgoing {
                graph
                    .forward_links
                    .entry(source_path.to_string())
                    .or_default()
                    .push(link.target_path.clone());
                graph
                    .backlinks
                    .entry(link.target_path.clone())
                    .or_default()
                    .push(source_path.to_string());
                if link.link_type == LinkType::Embed {
                    graph
                        .forward_embeds
                        .entry(source_path.to_string())
                        .or_default()
                        .push(link.target_path.clone());
                    graph
                        .back_embeds
                        .entry(link.target_path.clone())
                        .or_default()
                        .push(source_path.to_string());
                }
            }
        }
        graph
    }

    /// Pages `path` links to (any `LinkType`), in resolution order. Empty if
    /// `path` has no outgoing links or is not a source in this graph.
    pub fn forward_links(&self, path: &str) -> &[String] {
        self.forward_links.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Pages `path` embeds (`LinkType::Embed` only). Subset of `forward_links`.
    pub fn forward_embeds(&self, path: &str) -> &[String] {
        self.forward_embeds.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Pages that link to `path` (any `LinkType`). Empty if nothing links here.
    pub fn backlinks(&self, path: &str) -> &[String] {
        self.backlinks.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Pages that embed `path` (`LinkType::Embed` only). Subset of `backlinks`.
    pub fn back_embeds(&self, path: &str) -> &[String] {
        self.back_embeds.get(path).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(target: &str, link_type: LinkType) -> OutgoingLink {
        OutgoingLink {
            target_path: target.to_string(),
            display_text: target.to_string(),
            link_type,
        }
    }

    #[test]
    fn empty_graph_has_no_edges() {
        let graph = DepGraph::build(std::iter::empty());
        assert!(graph.backlinks("a.md").is_empty());
        assert!(graph.forward_links("a.md").is_empty());
    }

    #[test]
    fn forward_and_backlinks_are_reciprocal() {
        let a_links = [link("b.md", LinkType::Wikilink)];
        let graph = DepGraph::build([("a.md", a_links.as_slice())]);
        assert_eq!(graph.forward_links("a.md"), ["b.md"]);
        assert_eq!(graph.backlinks("b.md"), ["a.md"]);
        assert!(graph.backlinks("a.md").is_empty());
    }

    #[test]
    fn embed_edges_are_a_subset_of_link_edges() {
        let a_links = [
            link("b.md", LinkType::Wikilink),
            link("c.md", LinkType::Embed),
        ];
        let graph = DepGraph::build([("a.md", a_links.as_slice())]);
        assert_eq!(graph.forward_links("a.md"), ["b.md", "c.md"]);
        assert_eq!(graph.forward_embeds("a.md"), ["c.md"]);
        assert_eq!(graph.backlinks("c.md"), ["a.md"]);
        assert_eq!(graph.back_embeds("c.md"), ["a.md"]);
        assert!(graph.back_embeds("b.md").is_empty());
    }

    #[test]
    fn multiple_sources_linking_the_same_target_accumulate() {
        let a_links = [link("shared.md", LinkType::Wikilink)];
        let b_links = [link("shared.md", LinkType::Wikilink)];
        let graph = DepGraph::build([
            ("a.md", a_links.as_slice()),
            ("b.md", b_links.as_slice()),
        ]);
        assert_eq!(graph.backlinks("shared.md"), ["a.md", "b.md"]);
    }

    #[test]
    fn unknown_path_returns_empty_slice() {
        let graph = DepGraph::build(std::iter::empty());
        assert!(graph.forward_links("nowhere.md").is_empty());
        assert!(graph.forward_embeds("nowhere.md").is_empty());
        assert!(graph.back_embeds("nowhere.md").is_empty());
    }
}
