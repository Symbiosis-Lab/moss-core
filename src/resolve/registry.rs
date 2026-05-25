//! Extensible registry for embed renderers.
//!
//! Built-ins come from moss-core; plugins register at pipeline init via
//! [`RendererRegistryBuilder::with_boxed`]. The default lookup in
//! [`super::embed_renderer::lookup_renderer`] uses the built-in-only
//! registry; pipelines with plugin renderers build their own registry and
//! thread it through [`super::wikilinks::resolve_wikilinks_with_registry`].
//!
//! # Two-pass dispatch (plugin renderers)
//!
//! Plugin renderers are declared in a plugin's manifest (a `script` path
//! pointing at a JS function) but moss-core can't execute JavaScript. The
//! pipeline handles this with a two-pass design:
//!
//! ```text
//! Pass 1 (moss-core, pure):
//!   ![[diagram.dot]]
//!     → PluginEmbedRenderer::render(&parsed)     (src-tauri adapter)
//!     → RenderedEmbed::Deferred { marker: "<!-- moss-embed-plugin-graphviz:... -->" }
//!     → marker spliced into content
//!
//! Pass 2 (src-tauri, async + I/O):
//!   resolve_embeds_with_handlers scans for marker prefix
//!     → MarkerHandlers registry dispatches to plugin IPC
//!     → plugin script runs `dot -Tsvg`
//!     → returned HTML spliced back into content
//! ```
//!
//! The first pass stays pure; the second pass does I/O. Plugin author
//! writes one JS function; they never touch Rust.
//!
//! # Built-ins-win-on-collision
//!
//! Built-ins are registered first in [`RendererRegistry::builtin`]. Lookup
//! is first-match-wins, so a plugin declaring an extension that clashes
//! with a built-in (e.g., `.html`) can't shadow the built-in — its
//! renderer is registered in the registry but never dispatched. This is a
//! deliberate safety property; override policy (allow plugin to replace
//! built-in) is a future extension.
//!
//! # When to use which API
//!
//! | Pipeline type | Function | Registry used |
//! |---|---|---|
//! | No plugins | [`super::wikilinks::resolve_wikilinks`] | built-in only (via `lookup_renderer`) |
//! | With plugins | [`super::wikilinks::resolve_wikilinks_with_registry`] | custom registry built at init |

use super::embed_renderer::{
    EmbedRenderer, ImageRenderer, IframeRenderer, MarkdownEmbedRenderer,
    ModelViewerRenderer, NotebookRenderer, PdfRenderer, TableRenderer, VideoRenderer,
    AudioRenderer,
};

/// A registry of embed renderers, built from the built-in set plus any custom
/// renderers (typically plugin adapters) added at construction time.
///
/// Built in one pass at pipeline init and then treated as immutable. Lookup
/// is first-match-wins by extension; built-ins come first so plugins can't
/// shadow them.
pub struct RendererRegistry {
    renderers: Vec<&'static dyn EmbedRenderer>,
}

impl RendererRegistry {
    /// Start a builder seeded with the built-in renderers.
    pub fn builtin() -> RendererRegistryBuilder {
        RendererRegistryBuilder::new().with_builtins()
    }

    /// Empty builder (for tests).
    pub fn empty() -> RendererRegistryBuilder {
        RendererRegistryBuilder::new()
    }

    /// Look up a renderer by extension (case-insensitive, no leading dot).
    pub fn lookup(&self, ext: &str) -> Option<&'static dyn EmbedRenderer> {
        if ext.is_empty() {
            return None;
        }
        self.renderers
            .iter()
            .copied()
            .find(|r| r.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }

    /// All renderers in registration order (for diagnostics + head-asset walk).
    pub fn all(&self) -> &[&'static dyn EmbedRenderer] {
        &self.renderers
    }
}

/// Builder for [`RendererRegistry`].
pub struct RendererRegistryBuilder {
    renderers: Vec<&'static dyn EmbedRenderer>,
}

impl RendererRegistryBuilder {
    fn new() -> Self {
        Self {
            renderers: Vec::new(),
        }
    }

    fn with_builtins(mut self) -> Self {
        // Order matches embed_renderer::registry() — built-ins first so they
        // win on extension collision with plugins.
        self.renderers.push(&ImageRenderer);
        self.renderers.push(&MarkdownEmbedRenderer);
        self.renderers.push(&IframeRenderer);
        self.renderers.push(&PdfRenderer);
        self.renderers.push(&AudioRenderer);
        self.renderers.push(&VideoRenderer);
        self.renderers.push(&NotebookRenderer);
        self.renderers.push(&ModelViewerRenderer);
        self.renderers.push(&TableRenderer);
        self
    }

    /// Add a `'static` renderer (zero-size unit struct).
    pub fn with_static(mut self, r: &'static dyn EmbedRenderer) -> Self {
        self.renderers.push(r);
        self
    }

    /// Add a heap-allocated renderer (e.g., a plugin adapter).
    ///
    /// Leaks the `Box` to produce a `&'static dyn` reference. Acceptable
    /// because plugin registration happens at pipeline init (one-time, not
    /// hot path); one leak per plugin renderer is negligible.
    pub fn with_boxed(mut self, r: Box<dyn EmbedRenderer>) -> Self {
        let leaked: &'static dyn EmbedRenderer = Box::leak(r);
        self.renderers.push(leaked);
        self
    }

    /// Finalize the registry.
    pub fn build(self) -> RendererRegistry {
        RendererRegistry {
            renderers: self.renderers,
        }
    }
}

impl Default for RendererRegistry {
    fn default() -> Self {
        RendererRegistry::builtin().build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::embed_renderer::{ParsedEmbed, RenderedEmbed};

    #[derive(Debug)]
    struct CustomRenderer;
    impl EmbedRenderer for CustomRenderer {
        fn extensions(&self) -> &[&'static str] {
            &["xyz"]
        }
        fn render(&self, _: &ParsedEmbed<'_>) -> RenderedEmbed {
            RenderedEmbed::Inline("custom".to_string())
        }
    }

    #[test]
    fn test_builtin_registry_has_core_renderers() {
        let reg = RendererRegistry::builtin().build();
        for ext in ["jpg", "md", "html", "pdf", "mp3", "mp4", "ipynb", "glb", "csv"] {
            assert!(
                reg.lookup(ext).is_some(),
                "builtin missing renderer for .{}",
                ext
            );
        }
        assert!(reg.lookup("xyz").is_none());
    }

    #[test]
    fn test_builder_adds_custom_renderer() {
        let reg = RendererRegistry::builtin()
            .with_boxed(Box::new(CustomRenderer))
            .build();
        assert!(reg.lookup("xyz").is_some());
        assert!(reg.lookup("jpg").is_some(), "built-ins still present");
    }

    #[test]
    fn test_builtin_wins_on_collision() {
        // If a plugin tried to claim .html, the built-in IframeRenderer (added
        // first) wins because lookup is first-match.
        #[derive(Debug)]
        struct FakeHtmlRenderer;
        impl EmbedRenderer for FakeHtmlRenderer {
            fn extensions(&self) -> &[&'static str] {
                &["html"]
            }
            fn render(&self, _: &ParsedEmbed<'_>) -> RenderedEmbed {
                RenderedEmbed::Html("<fake></fake>".to_string())
            }
        }
        let reg = RendererRegistry::builtin()
            .with_boxed(Box::new(FakeHtmlRenderer))
            .build();
        let r = reg.lookup("html").expect("has renderer");
        let out = r.render(&ParsedEmbed {
            resolved_path: "x.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        // Phase 0 Stage 1: built-in IframeRenderer emits CommonMark link
        // markdown carrying `moss:kind=iframe`, not literal `<iframe>` HTML.
        match out {
            RenderedEmbed::Inline(s) => assert!(
                s.contains("kind=iframe"),
                "built-in iframe should win, got: {}",
                s
            ),
            _ => panic!("expected Inline (Stage 1 markdown) from built-in IframeRenderer"),
        }
    }

    #[test]
    fn test_empty_registry() {
        let reg = RendererRegistry::empty().build();
        assert!(reg.lookup("jpg").is_none());
    }

    #[test]
    fn test_lookup_case_insensitive() {
        let reg = RendererRegistry::builtin().build();
        assert!(reg.lookup("JPG").is_some());
        assert!(reg.lookup("Jpg").is_some());
    }

    #[test]
    fn test_all_returns_registered_renderers() {
        let reg = RendererRegistry::builtin().build();
        // 9 built-ins as of Phase D.
        assert_eq!(reg.all().len(), 9);
    }
}
