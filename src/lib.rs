//! **moss-core is the pure-Rust content engine behind [moss](https://mosspub.com),**
//! a desktop publishing app. It owns every transformation that turns a folder of
//! markdown into a website: parsing, wikilink resolution, HTML rendering,
//! frontmatter typing, and schema validation.
//!
//! Everything is **data in, data out** — strings and structs in; parsed ASTs,
//! diagnostics, and rendered HTML out. **Zero I/O, zero async, no global state.**
//! The filesystem, the network, and the async runtime all live one layer up, in
//! moss's host; this crate never touches them. That makes it deterministic,
//! trivially unit-testable, and embeddable in any Rust program — not just moss.
//!
//! # How it's laid out
//!
//! The modules cluster into four areas, plus the contract surface:
//!
//! - **Parse & render** — [`ast`] turns markdown into a typed tree (over
//!   `pulldown-cmark`) and renders it back to HTML through interceptable hooks;
//!   [`render`] emits media HTML (image/video/audio/iframe/pdf) for embeds.
//! - **Frontmatter & schema** — [`frontmatter`] parses YAML while preserving the
//!   body byte-for-byte; [`frontmatter_typed`] is the canonical `FrontMatter`
//!   struct; [`schema_fields`] is the single source of truth for built-in fields;
//!   [`validation`] produces LSP-style diagnostics against a schema.
//! - **Links & content model** — [`resolve`] is the one place wikilinks and
//!   embeds (`[[...]]`) become ordinary markdown links; [`content_graph`] does the
//!   Obsidian-style fuzzy path matching underneath.
//! - **Utilities** — small stateless helpers the editor and build share:
//!   [`slug`], [`date`], [`sort`], [`home`], [`page_kind`], and [`heading`].
//!
//! Plus [`contract`]: the design surface (W3C design tokens + the `moss-*` HTML
//! class table) that theme authors and codegen depend on.
//!
//! # Getting started
//!
//! Every entry point is a free function — pick the module and call it:
//!
//! ```
//! use moss_core::frontmatter;
//!
//! let raw = "---\ntitle: Hello\n---\n\nBody text";
//! let doc = frontmatter::parse(raw);
//! assert_eq!(doc.frontmatter.get("title").and_then(|v| v.as_str()), Some("Hello"));
//! assert_eq!(doc.body.trim(), "Body text"); // body preserved verbatim
//! ```
//!
//! From there: [`ast`] for the body tree, [`resolve`] to flatten wikilinks,
//! [`validation`] to lint frontmatter, and [`heading`] for anchors.
//!
//! # Guarantees
//!
//! Total functions: bad input degrades to a best-effort value, never an `Err` or
//! a panic. No `unsafe` (`#![forbid(unsafe_code)]`). Schema problems are reported
//! out-of-band as [`validation`] diagnostics, not return values.
//!
//! moss ships this crate in a host built with `panic = "abort"` (release
//! profile), so a panic on user input crashes the whole desktop app (see the
//! `date.rs` fix for the
//! editor-mount panic on Chinese filenames). The lint attributes below enforce
//! the panic-free contract — `deny(clippy::string_slice)` plus
//! `deny(clippy::unwrap_used/expect_used)` outside tests — each with a per-site
//! escape-hatch rule.

#![forbid(unsafe_code)]
// `clippy::string_slice` flags `&s[..n]` byte-indexed slicing on `&str`. That
// pattern crashed the editor on `纽约诸法门.md` — `len() < 10` is bytes, not
// chars, so the guard let the slice cut inside `法`. Safe call sites must
// carry a per-site `#[allow(clippy::string_slice)]` with a one-line rationale
// (e.g. "char-aligned: pos came from `find('/')`"). Audited at PR time, not
// "we hope no one writes the bug shape again."
#![deny(clippy::string_slice)]
// `clippy::unwrap_used` / `clippy::expect_used` enforce the second half of the
// panic-free contract: production code must never `.unwrap()` / `.expect()`
// a value that could be `None`/`Err` at runtime. Test code (`#[cfg(test)]
// mod tests`) is exempted via `cfg_attr(not(test), ...)` because tests
// legitimately want to fail fast on assertion violations. Safe call sites
// must annotate with `#[allow(clippy::unwrap_used)]` + per-site rationale,
// same pattern as `clippy::string_slice`.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod ast;
pub mod asset_paths;
pub(crate) mod path_ext;
pub mod asset_snapshot;
pub mod content_graph;
pub mod contract;
pub mod csv_table;
pub mod date;
pub mod home;
pub mod frontmatter;
pub mod frontmatter_union;
pub mod frontmatter_typed;
pub mod heading;
pub use heading::{extract_headings, HeadingInfo};
pub mod link_candidates;
pub mod link_completions;
pub mod media;
pub mod page_kind;
pub use page_kind::PageKind;
pub mod render;
pub mod resolve;
pub mod resolved;
pub use resolved::{Resolved, ResolvedOrigin};
pub mod schema;
pub mod schema_fields;
pub mod slug;
pub mod sort;
pub mod shortcode_tokens;
pub mod validation;
