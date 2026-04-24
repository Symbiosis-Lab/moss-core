//! moss-core: Pure Rust content processing.
//!
//! Zero I/O, zero async. Takes strings in, returns strings out.
//! All filesystem access happens in the Tauri layer.

#![forbid(unsafe_code)]

pub mod content_graph;
pub mod csv_table;
pub mod home;
pub mod frontmatter;
pub mod link_candidates;
pub mod heading_anchor;
pub mod media;
pub mod page_kind;
pub use page_kind::PageKind;
pub mod resolve;
pub mod schema;
pub mod schema_fields;
pub mod shortcode_tokens;
pub mod validation;
