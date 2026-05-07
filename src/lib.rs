//! moss-core: Pure Rust content processing.
//!
//! Zero I/O, zero async. Takes strings in, returns strings out.
//! All filesystem access happens in the Tauri layer.
//!
//! **Panic-free contract:** Every public function in moss-core is invoked from
//! Tauri command handlers in the host process. The host is configured with
//! `panic = "abort"` for release builds, so any panic on user input crashes
//! the whole desktop app (see fix in `date.rs` for the editor-mount panic on
//! Chinese filenames). Treat moss-core as panic-free on user input: never
//! `unwrap`/`expect` a value derived from arbitrary user data, and never use
//! byte-indexed `&str` slicing without a `char_boundary` guarantee.

#![forbid(unsafe_code)]
// `clippy::string_slice` flags `&s[..n]` byte-indexed slicing on `&str`. That
// pattern crashed the editor on `纽约诸法门.md` — `len() < 10` is bytes, not
// chars, so the guard let the slice cut inside `法`. Safe call sites must
// carry a per-site `#[allow(clippy::string_slice)]` with a one-line rationale
// (e.g. "char-aligned: pos came from `find('/')`"). Audited at PR time, not
// "we hope no one writes the bug shape again."
#![deny(clippy::string_slice)]

pub mod ast;
pub mod content_graph;
pub mod csv_table;
pub mod date;
pub mod home;
pub mod frontmatter;
pub mod heading;
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
