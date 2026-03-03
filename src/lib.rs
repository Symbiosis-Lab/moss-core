//! moss-core: Pure Rust content processing.
//!
//! Zero I/O, zero async. Takes strings in, returns strings out.
//! All filesystem access happens in the Tauri layer.

pub mod completion;
pub mod content_graph;
pub mod frontmatter;
pub mod heading_anchor;
pub mod resolve;
pub mod schema;
pub mod shortcode;
pub mod validation;
