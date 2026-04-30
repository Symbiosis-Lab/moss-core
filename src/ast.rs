//! Typed body AST.
//!
//! Companion to [`crate::frontmatter`] (typed frontmatter) and
//! [`crate::resolve`] (the upstream wikilink/embed resolver). This module
//! owns the parsed body of a markdown document as a closed enum tree and
//! the visitor + render-hooks machinery that walks it.
//!
//! Pipeline order: the upstream `resolve` phase has already rewritten the
//! markdown source so wikilinks `[[foo]]` are now standard markdown links
//! `[foo](moss-resolved:foo.md)`. From here:
//!
//! ```text
//!   markdown source (post-resolve)
//!         │
//!         ▼
//!     parser::parse  →  Document (typed AST with Url::Unresolved)
//!         │
//!         ▼  visit::visit_urls_mut(&mut doc, &resolver)
//!     Document with every Url in the Resolved state
//!         │
//!         ▼  render::render_document(&doc, &hooks)
//!     final HTML
//! ```
//!
//! Design principles (from `docs/architecture/typed-body-ast.md`):
//!
//! - The AST is data, not a hierarchy of objects. Pattern matching is the
//!   visitor framework.
//! - URL resolution is a typed state machine. The renderer accepts only
//!   `Url::Resolved`; emitting `Url::Unresolved` is a bug.
//! - moss-core stays pure Rust: zero I/O, zero async.

pub mod document;
pub mod node;
pub mod parser;
pub mod shortcode;
pub mod url;

pub use document::Document;
pub use node::{Block, Inline};
pub use parser::parse;
pub use shortcode::{Shortcode, ShortcodeKind};
pub use url::{ResolvedUrl, Url, UrlKind};
