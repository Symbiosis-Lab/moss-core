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

pub mod attrs;
pub mod cells;
pub mod dispatch_wikilink_embeds;
pub mod document;
pub mod editor_scan;
pub mod extract_hero;
pub mod hooks;
pub mod node;
pub mod parser;
pub mod query;
pub mod render;
pub mod resolve_urls;
pub mod shortcode;
pub mod shortcode_extract;
pub mod url;
pub mod visit;

pub use dispatch_wikilink_embeds::{dispatch_wikilink_embeds, WikilinkDispatchResult};
pub use document::Document;
pub use extract_hero::{extract_hero, HeroExtraction};
pub use hooks::{DefaultHooks, RenderHooks};
pub use node::{Block, CalloutKind, Fold, Inline};
pub use parser::parse;
pub use query::find_first_block_image;
pub use render::render_document;
pub use resolve_urls::{classify_remaining_urls, resolve_urls};
pub use shortcode::{
    ButtonItem, ButtonsShortcode, GalleryItem, GalleryShortcode, GridShortcode, HeroShortcode,
    RecentShortcode, Shortcode, ShortcodeKind, SubscribeShortcode,
};
pub use url::{ResolvedUrl, Url, UrlKind};
pub use visit::{has_shortcode_recursive, visit_blocks, visit_urls_mut};
