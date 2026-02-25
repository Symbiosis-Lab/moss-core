//! Link resolution types and fuzzy path resolution.
//!
//! This module provides shared types for the resolve phase of the
//! compilation pipeline, plus a fuzzy path resolver that wraps
//! [`ContentGraph::resolve_path`](crate::content_graph::ContentGraph::resolve_path).

pub mod block_refs;
pub mod callouts;
pub mod fuzzy_path;
pub mod wikilinks;

/// A link going out from a document.
#[derive(Debug, Clone)]
pub struct OutgoingLink {
    pub target_path: String,
    pub display_text: String,
    pub link_type: LinkType,
}

/// The kind of link syntax used.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkType {
    /// `[[target]]` or `[[target|display]]`
    Wikilink,
    /// `![[target]]` — an embedded/transcluded reference
    Embed,
    /// Standard markdown `[text](url)`
    Standard,
}

/// A diagnostic message from the resolve phase.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub source_path: String,
    pub reference: String,
}
