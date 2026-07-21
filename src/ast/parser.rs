//! Pulldown-cmark → typed AST parser.
//!
//! Walks `pulldown_cmark::Event` and assembles a [`Document`]. The parser
//! enables the same extensions moss's pipeline does: tables, footnotes,
//! strikethrough.
//!
//! All URL nodes start as [`Url::Unresolved`]; classifying into
//! [`Url::Resolved`] is the job of [`crate::ast::visit::visit_urls_mut`]
//! (a separate pass).
//!
//! Heading IDs ARE assigned by this parser. Phase 4 PR2: each
//! `Tag::Heading` arm computes the Obsidian-compatible anchor slug from
//! the heading's text content (only `Event::Text` / `Event::Code`,
//! matching production's `transform_events` behavior in
//! `src-tauri/src/build/markdown/pipeline.rs` lines 1776-1845); a
//! post-parse pass ([`assign_heading_id_suffixes`]) walks all headings in
//! document order (recursively into BlockQuotes, lists, callouts) and
//! applies duplicate-suffix numbering (`{slug}-1`, `-2`, …) matching the
//! `id_counts` HashMap behavior at `pipeline.rs:1798`.

use std::collections::HashMap;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::document::{BlockMeta, Document};
use super::math_text::{math_inline, math_source};
use super::node::{Block, CalloutKind, Fold, Inline};
use super::shortcode_extract::{extract_shortcodes_with_config, parse_placeholder, ExtractedShortcode};
use super::url::Url;
use crate::heading_anchor::obsidian_heading_anchor;

/// Parser configuration flags.
///
/// Threaded through [`parse_with_config`] to gate optional parser behaviors
/// that the renderer needs to coordinate with (source-line tracking for
/// preview scroll sync, implicit-figure promotion).
///
/// [`Default`] = "production preview off" — `emit_source_lines: false`,
/// `implicit_figure: true`. The `implicit_figure` default mirrors today's
/// always-on behavior of the parser before this config existed; flipping it
/// off is opt-in for the small set of fragment-render call sites that need
/// bare `<img>` (none today, but the flag exists for symmetry with the
/// legacy `transform_events` API and the production `site_config` field).
#[derive(Debug, Clone, Copy)]
pub struct ParseConfig {
    /// When true, populates [`BlockMeta::source_line`] for top-level
    /// blocks. The renderer emits `data-source-line="N"` on the opening
    /// tag for any block whose meta carries `Some(N)`.
    ///
    /// Production wires this from `process_markdown_file`'s
    /// `emit_source_lines` argument (`true` during preview builds, `false`
    /// during ship-stage publish builds — `data-source-line` is stripped
    /// at ship time anyway, but emitting fewer attrs upstream is cheaper
    /// and keeps published HTML clean from earlier stages).
    pub emit_source_lines: bool,

    /// When true (default), image-only paragraphs promote to
    /// [`Block::Figure`] via [`try_promote_to_figure`]. When false, they
    /// stay as [`Block::Paragraph`] containing one [`Inline::Image`].
    ///
    /// Production wires this from `site_config.implicit_figure` (default
    /// `true`). The flag mirrors the legacy `transform_events`
    /// implicit-figure pass: sites that prefer bare `<img>` (no `<figure>`
    /// wrap) can opt out.
    pub implicit_figure: bool,

    /// Added to every computed `source_line` so the emitted
    /// `data-source-line` / `data-source-range` values match the editor's
    /// REAL FILE line numbers (CM6 `doc.lineAt`), not body-relative lines.
    ///
    /// The parser only ever sees the markdown BODY (frontmatter is stripped
    /// upstream), so its byte offsets — and thus `LineLookup` — are
    /// body-relative. The editor, however, reports raw-file lines including
    /// the frontmatter. Without this offset, every annotation is short by the
    /// frontmatter line count, so editor→preview scroll-sync maps to the wrong
    /// element (the home page's grid scrolled the preview to the bottom). Set
    /// to the number of lines the frontmatter consumes (0 when there is none).
    /// See `process_markdown_file` and docs/architecture/editor-preview-sync.md
    /// "Known defect — source-line coordinate-system mismatch".
    pub source_line_offset: usize,

    /// When true, `$…$` / `$$…$$` parse as math ([`Options::ENABLE_MATH`])
    /// and render as escaped LaTeX source in `<code class="moss-math">`.
    /// When false (default), `$` is an ordinary character and math source
    /// passes through as literal text.
    ///
    /// Default is `false` — unlike the other flags, this one changes what
    /// the *characters* mean, so every in-crate `parse()` caller and every
    /// committed snapshot fixture keeps today's behavior until a site opts
    /// in. Production wires it from `site_config.math` (`[site].math`,
    /// default on), which is where the "is `$5` currency or an unclosed
    /// equation?" judgment belongs.
    pub math: bool,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            emit_source_lines: false,
            // `true` matches today's always-on behavior of the parser
            // before ParseConfig existed; the ~40 in-crate `parse()`
            // callers all assume figure promotion happens.
            implicit_figure: true,
            source_line_offset: 0,
            // Off by default so the ~40 in-crate `parse()` callers and every
            // committed snapshot fixture are untouched by math landing.
            // Production opts in via `[site].math`.
            math: false,
        }
    }
}

/// **The** pulldown-cmark option set moss parses markdown with.
///
/// Every parser construction site in the repo must call this rather than
/// hand-assembling its own `Options` — moss previously had five independent
/// `Options` blocks (typed AST, newsletter ×2, `llms_txt`, the markdown
/// pipeline), and each one that drifted became a surface where the same
/// document parsed differently depending on which output it was headed for.
/// A site that legitimately needs a different set (the newsletter walker
/// deliberately omits `ENABLE_FOOTNOTES`, because footnote backlinks are
/// meaningless in an inbox) calls this and then removes the one option, so
/// the divergence reads as an explicit delta at the call site instead of
/// being invisibly re-hand-rolled.
///
/// `math` gates `ENABLE_MATH` (`$…$` / `$$…$$` → [`Event::InlineMath`] /
/// [`Event::DisplayMath`]). It is a parameter rather than part of the base
/// set because it changes the meaning of a character that appears in
/// ordinary prose (`$5`), so it is the one option a site must opt into —
/// production wires it from `[site].math` on `SiteConfig`.
///
/// **Enabling `math` obliges the caller's event walker to handle both math
/// events.** pulldown emits them as leaf inline events; a walker that
/// pattern-matches known events and ignores the rest will *silently delete*
/// every equation in the document (measured: `Energy $E = mc^2$.` →
/// `<p>Energy .</p>`). See `src-tauri/tests/math_wiring_invariant_test.rs`,
/// which fails any site that turns math on without arms in the same walker.
pub fn parser_options(math: bool) -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    // Phase 3 PR2: pulldown-cmark emits `LinkType::WikiLink` events for
    // `[[…]]` / `![[…]]` natively. The typed-AST parser preserves them as
    // `Inline::Link`/`Inline::Image` with `Url::Unresolved`; resolution
    // happens in the later `visit_urls_mut` pass.
    options.insert(Options::ENABLE_WIKILINKS);
    if math {
        options.insert(Options::ENABLE_MATH);
    }
    options
}

/// Parse markdown into a typed [`Document`] using the default config.
///
/// Equivalent to `parse_with_config(markdown, &ParseConfig::default())`.
/// This is the entry point for the ~40 in-crate callers that don't need
/// per-parse configuration (URL resolution tests, frontmatter round-trip
/// tests, etc.). Production paths that need source-line tracking or
/// implicit-figure toggling call [`parse_with_config`].
pub fn parse(markdown: &str) -> Document {
    parse_with_config(markdown, &ParseConfig::default())
}

/// Parse markdown into a typed [`Document`].
///
/// This is the AST entry point. The input is post-resolve markdown (the
/// upstream resolve pipeline has already rewritten wikilinks into standard
/// markdown links with `moss-resolved:` prefixes).
///
/// Two-stage parse:
/// 1. [`extract_shortcodes`] pre-scans for `:::name` blocks, replacing
///    each with a sentinel HTML comment.
/// 2. Pulldown-cmark parses the substituted markdown into events; each
///    sentinel comes back as a `Block::Other` raw HTML.
/// 3. A final pass walks the AST and substitutes `Block::Other` sentinel
///    payloads with the corresponding typed [`Block::Shortcode`].
///
/// When `config.emit_source_lines` is true, the parser walks events via
/// `into_offset_iter()` so each top-level block carries the byte offset
/// of its first event; a [`LineLookup`] converts the offset to a 1-based
/// line number stored in [`BlockMeta::source_line`].
pub fn parse_with_config(markdown: &str, config: &ParseConfig) -> Document {
    let extraction = extract_shortcodes_with_config(markdown, config);

    let options = parser_options(config.math);

    // Source-line tracking requires the `into_offset_iter` form of the
    // parser, which yields (Event, Range<usize>). When tracking is off,
    // we use the plain iterator (no per-event offset overhead).
    let (events, offsets): (Vec<Event<'_>>, Vec<Option<std::ops::Range<usize>>>) =
        if config.emit_source_lines {
            let mut evs = Vec::new();
            let mut offs = Vec::new();
            for (event, range) in
                Parser::new_ext(&extraction.markdown_with_placeholders, options).into_offset_iter()
            {
                evs.push(event);
                offs.push(Some(range));
            }
            (evs, offs)
        } else {
            let evs: Vec<Event<'_>> =
                Parser::new_ext(&extraction.markdown_with_placeholders, options).collect();
            let len = evs.len();
            (evs, vec![None; len])
        };

    // Build the prefix-sum line table once (only when needed).
    //
    // CAVEAT: the markdown that the offsets index into is
    // `extraction.markdown_with_placeholders`, NOT the original
    // `markdown` passed in. Shortcode extraction may rewrite some bytes
    // into sentinel HTML comments of a different length; line numbers
    // would be off for blocks following an extracted shortcode if we
    // built the lookup against the original. We build against the
    // post-extraction string, so the line numbers match the
    // post-extraction view — which is what users see in their editor
    // before shortcode-block lines, and is "close enough" after (the
    // sentinel preserves one line per extracted block, so line counts
    // after the extracted block are within one of the source). See the
    // architecture note in `shortcode_extract.rs` for the placeholder
    // shape.
    //
    // For the source-line-off path, lookup is unused.
    let line_lookup = if config.emit_source_lines {
        Some(LineLookup::build(
            &extraction.markdown_with_placeholders,
            config.source_line_offset,
        ))
    } else {
        None
    };

    // Line-tracking context handed to every recursive parser entry; the
    // Tag::List / Tag::Table arms consult it to annotate per-item / per-row
    // source lines. `None` when `emit_source_lines` is off; the inner
    // arms see this as "skip annotation" and emit empty parallel vecs.
    let line_ctx: Option<LineCtx<'_>> = line_lookup.as_ref().map(|lookup| LineCtx {
        lookup,
        offsets: &offsets,
    });

    let mut blocks = Vec::new();
    let mut block_meta: Vec<BlockMeta> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let event_start_idx = i;
        let (block, advance) = parse_block(&events, i, line_ctx.as_ref());
        if let Some(b) = block {
            // Compute source_line from the first event's byte offset, if
            // we collected offsets and a lookup is in scope.
            let source_line = match (line_lookup.as_ref(), offsets.get(event_start_idx)) {
                (Some(lookup), Some(Some(range))) => Some(lookup.line_at(range.start)),
                _ => None,
            };
            blocks.push(b);
            block_meta.push(BlockMeta { source_line });
        }
        i += advance.max(1);
    }

    // Substitute sentinel placeholders with their typed Shortcode variants.
    substitute_shortcode_placeholders(&mut blocks, &extraction.nonce, &extraction.extracted);

    // Implicit-figure gating: the per-paragraph `try_promote_to_figure`
    // inside `parse_block_with_tag` always runs (so the figure promotion
    // happens at parse time inside the Tag::Paragraph arm). When
    // `config.implicit_figure` is false, we walk the assembled blocks
    // and "undo" the promotion — converting `Block::Figure { image, ..}`
    // back to `Block::Paragraph(vec![image])`.
    //
    // The unwinding-at-the-end approach was chosen over threading the
    // flag into `parse_block_with_tag` because the latter would mean
    // propagating `config` through ~14 inner parser functions whose
    // signatures are already tight. The unwind is O(N) and only fires
    // on the rare opt-out path; production keeps the default `true`.
    if !config.implicit_figure {
        for block in blocks.iter_mut() {
            unwrap_implicit_figure(block);
        }
    }

    // Apply duplicate-suffix numbering to heading IDs in document order.
    // Each Tag::Heading arm computes the base slug; this pass disambiguates
    // collisions across the whole document, matching production's id_counts
    // HashMap behavior in pipeline.rs::transform_events.
    assign_heading_id_suffixes(&mut blocks);

    Document::from_blocks_with_meta(blocks, block_meta)
}

/// Recursively undo implicit-figure promotion in `block` and its children.
///
/// Called when `ParseConfig::implicit_figure` is false. Walks the block
/// tree (descending into containers — `BlockQuote`, `Callout`, `List`,
/// `LinkCard`) and rewrites any `Block::Figure` back to
/// `Block::Paragraph(vec![image])` with the original alt text preserved.
/// The caption is discarded (matches the legacy bare-`<img>` shape).
fn unwrap_implicit_figure(block: &mut Block) {
    // Replace this block if it's a Figure.
    if let Block::Figure { image, .. } = block {
        let img = std::mem::replace(
            image,
            Inline::Text(String::new()), // placeholder, overwritten below
        );
        *block = Block::Paragraph(vec![img]);
        return;
    }
    // Recurse into containers.
    match block {
        Block::BlockQuote(children) => {
            for child in children.iter_mut() {
                unwrap_implicit_figure(child);
            }
        }
        Block::Callout { children, .. } => {
            for child in children.iter_mut() {
                unwrap_implicit_figure(child);
            }
        }
        Block::List { items, .. } => {
            for item in items.iter_mut() {
                for child in item.iter_mut() {
                    unwrap_implicit_figure(child);
                }
            }
        }
        Block::LinkCard { children, .. } => {
            for child in children.iter_mut() {
                unwrap_implicit_figure(child);
            }
        }
        _ => {}
    }
}

/// Bundle of borrowed line-tracking state threaded through recursive
/// parser entries. Constructed once per `parse_with_config` when
/// `emit_source_lines` is true; `None` everywhere else.
///
/// `parse_block` / `parse_block_with_tag` consult `line_at_event` to
/// annotate per-`<li>` and per-`<tr>` source lines. The outer
/// top-level-block source line is computed at the parse loop itself
/// (already in place), not here.
struct LineCtx<'a> {
    lookup: &'a LineLookup,
    offsets: &'a [Option<std::ops::Range<usize>>],
}

impl<'a> LineCtx<'a> {
    /// 1-based source line of the event at `event_index`, or `None` if
    /// the offset is missing (defensive — shouldn't happen when the
    /// parser is operating with `emit_source_lines: true`).
    fn line_at_event(&self, event_index: usize) -> Option<usize> {
        match self.offsets.get(event_index) {
            Some(Some(range)) => Some(self.lookup.line_at(range.start)),
            _ => None,
        }
    }
}

/// Prefix-sum line-number lookup for byte offsets in a source string.
///
/// Built once per parse (when `emit_source_lines` is on). Stores the byte
/// offset of every `\n` in `source`; `line_at(offset)` returns the
/// 1-based line number containing that offset via binary search.
///
/// Equivalent (slower) form: `source[..offset].matches('\n').count() + 1`
/// — O(N) per call vs. O(log N) here. For documents with ~25 blocks the
/// difference is negligible, but the binary-search form is the canonical
/// pattern and is the cheaper hot-path shape.
struct LineLookup {
    /// Byte offsets of every `\n` in the source. Sorted ascending by
    /// construction. `newline_offsets[i]` is the byte index of the i-th
    /// newline (0-based).
    newline_offsets: Vec<usize>,
    /// Added to every `line_at` result so body-relative lines become
    /// raw-file lines (the frontmatter line count). See
    /// `ParseConfig::source_line_offset`.
    line_offset: usize,
}

impl LineLookup {
    fn build(source: &str, line_offset: usize) -> Self {
        let mut newline_offsets = Vec::new();
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                newline_offsets.push(i);
            }
        }
        Self {
            newline_offsets,
            line_offset,
        }
    }

    /// 1-based line number containing `byte_offset`, plus `line_offset`.
    ///
    /// Offset 0 (before any newline) → line 1. After the first newline →
    /// line 2. Etc. Offsets past the end of the source clamp to the last
    /// line + 1. `line_offset` (the frontmatter line count) is added so the
    /// result is a raw-file line, matching the editor's `doc.lineAt`.
    fn line_at(&self, byte_offset: usize) -> usize {
        // Find the number of newlines strictly before `byte_offset`.
        // That count + 1 is the 1-based line number.
        let body_line = match self.newline_offsets.binary_search(&byte_offset) {
            // Exact match: offset IS a newline byte; the newline belongs
            // to the line that ENDS at it, so line number = idx + 1.
            // (The next byte starts line idx + 2; this matches the legacy
            // count-and-add-1 semantics, which counts newlines BEFORE the
            // offset.)
            Ok(idx) => idx + 1,
            Err(idx) => idx + 1,
        };
        body_line + self.line_offset
    }
}

/// Walk top-level blocks; replace any `Block::Other` whose payload is a
/// `<!--MOSS_SC_{nonce}_{index}-->` sentinel with the corresponding typed
/// [`Block::Shortcode`].
fn substitute_shortcode_placeholders(
    blocks: &mut Vec<Block>,
    nonce: &str,
    extracted: &[ExtractedShortcode],
) {
    for block in blocks.iter_mut() {
        if let Block::Other(html) = block {
            if let Some(index) = parse_placeholder(nonce, html) {
                if let Some(entry) = extracted.iter().find(|e| e.index == index) {
                    *block = Block::Shortcode(entry.shortcode.clone());
                }
            }
        }
        // Future: descend into BlockQuote / List items / Callouts when
        // shortcodes inside those constructs are modeled. Phase B Tasks
        // 7-10 only need top-level shortcodes.
    }
}

/// Parse one block-level construct starting at `events[start]`. Returns
/// the parsed block (or `None` if `events[start]` was a closing tag /
/// stray event we skip) and how many events to advance.
///
/// `line_ctx` carries the optional line-tracking context for per-item
/// (`<li>`) and per-row (`<tr>`) source-line annotation; threaded through
/// to `parse_block_with_tag`.
fn parse_block(
    events: &[Event<'_>],
    start: usize,
    line_ctx: Option<&LineCtx<'_>>,
) -> (Option<Block>, usize) {
    match &events[start] {
        Event::Start(tag) => parse_block_with_tag(events, start, tag, line_ctx),
        Event::Text(_) | Event::Code(_) | Event::Html(_) | Event::SoftBreak | Event::HardBreak => {
            // Top-level stray inlines: pulldown-cmark always wraps these in
            // `Tag::Paragraph` at top level, so this branch is dead in practice.
            //
            // The tight-list-item case where the inlines are emitted directly
            // (no Tag::Paragraph wrap) was the load-bearing reason this branch
            // looked relevant; PR0.6 moved that responsibility into
            // `collect_item_blocks`, which synthesizes a Block::Paragraph for
            // stray inlines inside Tag::Item. See parser.rs's collect_item_blocks
            // helper.
            (None, 1)
        }
        Event::End(_) => (None, 1),
        Event::Rule => (Some(Block::ThematicBreak), 1),
        _ => (None, 1),
    }
}

fn parse_block_with_tag(
    events: &[Event<'_>],
    start: usize,
    tag: &Tag<'_>,
    line_ctx: Option<&LineCtx<'_>>,
) -> (Option<Block>, usize) {
    match tag {
        Tag::Heading { level, .. } => {
            let (children, end) = collect_inlines_until(events, start + 1, |e| {
                matches!(e, Event::End(TagEnd::Heading(_)))
            });
            let level_num = match level {
                HeadingLevel::H1 => 1,
                HeadingLevel::H2 => 2,
                HeadingLevel::H3 => 3,
                HeadingLevel::H4 => 4,
                HeadingLevel::H5 => 5,
                HeadingLevel::H6 => 6,
            };
            // Phase 4 PR2: compute the heading-anchor base slug from the
            // text/code content between Start(Heading) and End(Heading),
            // matching production's transform_events behavior. Inline HTML
            // (`<br>` etc.), images, and link href text are NOT included —
            // only Event::Text and Event::Code. The post-parse
            // `assign_heading_id_suffixes` pass disambiguates collisions.
            let heading_text = collect_heading_text(events, start + 1, end);
            let base_slug = obsidian_heading_anchor(&heading_text);
            (
                Some(Block::Heading {
                    level: level_num,
                    children,
                    id: Some(base_slug),
                }),
                end - start + 1,
            )
        }
        Tag::Paragraph => {
            let (children, end) = collect_inlines_until(events, start + 1, |e| {
                matches!(e, Event::End(TagEnd::Paragraph))
            });
            // Phase 4 PR3 (2026-05-27): detect image-only paragraphs and
            // promote to `Block::Figure`. See shape-spec § 1 detection
            // rule: exactly one `Inline::Image` plus any number of
            // whitespace-only `Inline::Text` / `Inline::LineBreak`
            // siblings qualifies. Caption defaults to the image's alt
            // text (mirroring transform_events' implicit-figure path);
            // empty alt yields `caption: None` so no `<figcaption>` is
            // emitted.
            //
            // A paragraph with image+prose (e.g. `![img](src) caption text`)
            // does NOT qualify; it stays as `Block::Paragraph`. This is the
            // critical regression guard — see PR1 v2 (commit 71c657af3)
            // for the analogous shape decision at the inline image hook
            // level: inline images use `MarkdownInline` (no figure wrap);
            // only the standalone figure case here uses the figure wrap.
            let block = match try_promote_to_figure(children) {
                Ok(figure) => figure,
                Err(original_inlines) => Block::Paragraph(original_inlines),
            };
            (Some(block), end - start + 1)
        }
        Tag::CodeBlock(kind) => {
            let lang = match kind {
                pulldown_cmark::CodeBlockKind::Fenced(s) if !s.is_empty() => Some(s.to_string()),
                _ => None,
            };
            let mut value = String::new();
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::CodeBlock) => break,
                    Event::Text(t) => value.push_str(t),
                    _ => {}
                }
                i += 1;
            }
            (Some(Block::CodeBlock { lang, value }), i - start + 1)
        }
        Tag::BlockQuote(_) => {
            // Phase 4 PR4: detect Obsidian-style callouts. A blockquote
            // whose first paragraph's leading text matches `[!<kind>]`
            // (with optional `+`/`-` foldable suffix and optional
            // inline title) promotes to `Block::Callout`. Otherwise it
            // stays a plain blockquote. See shape-spec § 1.
            //
            // Detection works on the EVENT stream (not the parsed
            // children) because pulldown-cmark's SoftBreak events
            // become `Inline::Text("\n")` during inline parsing
            // (PR4.5 aligned to CommonMark spec — see
            // `parse_inline` SoftBreak handling). Working on events
            // preserves the structural break before inline
            // collapse, which is what the marker-line-vs-body-line
            // boundary check needs.
            match detect_and_assemble_callout(events, start + 1, line_ctx) {
                Some((block, body_end)) => (Some(block), body_end - start + 1),
                None => {
                    let (children, end) = collect_blocks_until(events, start + 1, line_ctx, |e| {
                        matches!(e, Event::End(TagEnd::BlockQuote(_)))
                    });
                    (Some(Block::BlockQuote(children)), end - start + 1)
                }
            }
        }
        Tag::List(start_num) => {
            let ordered = start_num.is_some();
            // Preserve explicit ordered-list start number when it's not
            // the implicit default `1`. `3. foo` → `Some(3)` so the
            // renderer can emit `<ol start="3">`. pulldown-cmark
            // normalizes `1. foo` to `Some(1)`, which we collapse to
            // `None` because `<ol>` and `<ol start="1">` are
            // semantically identical and we prefer the cleaner attr-free
            // shape for the common case. Bound name is `list_start` to
            // avoid shadowing the outer `start: usize` event-index
            // parameter.
            let list_start = match start_num {
                Some(n) if *n != 1 => Some(*n),
                _ => None,
            };
            let mut items: Vec<Vec<Block>> = Vec::new();
            // Parallel-to-`items` per-`<li>` source-line annotations.
            // Empty when `line_ctx` is None; otherwise tracks each
            // `Event::Start(Tag::Item)`'s byte offset → line. The renderer
            // emits `<li data-source-line="N">` for entries that are Some.
            let mut item_source_lines: Vec<Option<usize>> = Vec::new();
            let track_lines = line_ctx.is_some();
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::List(_)) => break,
                    Event::Start(Tag::Item) => {
                        if track_lines {
                            item_source_lines.push(line_ctx.and_then(|ctx| ctx.line_at_event(i)));
                        }
                        let (item_blocks, end) = collect_item_blocks(events, i + 1, line_ctx);
                        items.push(item_blocks);
                        i = end + 1;
                    }
                    _ => i += 1,
                }
            }
            (
                Some(Block::List {
                    ordered,
                    start: list_start,
                    items,
                    item_source_lines,
                }),
                i - start + 1,
            )
        }
        Tag::Table(_) => {
            let mut header: Vec<Vec<Inline>> = Vec::new();
            let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
            // Per-`<tr>` source-line tracking. `header_source_line` is the
            // `<thead><tr>` line; `row_source_lines` is parallel to `rows`.
            // Both stay empty / None when `line_ctx` is None.
            let mut header_source_line: Option<usize> = None;
            let mut row_source_lines: Vec<Option<usize>> = Vec::new();
            let track_lines = line_ctx.is_some();
            let mut current_row: Vec<Vec<Inline>> = Vec::new();
            let mut in_head = false;
            let mut in_body_row = false;
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::Table) => break,
                    Event::Start(Tag::TableHead) => {
                        in_head = true;
                        // pulldown-cmark does NOT emit `Tag::TableRow` for the
                        // header row — it goes straight from `Tag::TableHead`
                        // to the cells. So we anchor the header `<tr>` line
                        // to the `TableHead` event itself (line of the
                        // markdown `| h |` row).
                        if track_lines {
                            header_source_line = line_ctx.and_then(|ctx| ctx.line_at_event(i));
                        }
                        i += 1;
                    }
                    Event::End(TagEnd::TableHead) => {
                        in_head = false;
                        i += 1;
                    }
                    Event::Start(Tag::TableRow) => {
                        in_body_row = true;
                        current_row = Vec::new();
                        if track_lines {
                            // pulldown-cmark only emits `TableRow` for body
                            // rows (header cells live directly inside
                            // `TableHead`). Always push to body lines here.
                            row_source_lines.push(line_ctx.and_then(|ctx| ctx.line_at_event(i)));
                        }
                        i += 1;
                    }
                    Event::End(TagEnd::TableRow) => {
                        if in_body_row {
                            rows.push(std::mem::take(&mut current_row));
                            in_body_row = false;
                        }
                        i += 1;
                    }
                    Event::Start(Tag::TableCell) => {
                        let (cell_inlines, end) = collect_inlines_until(events, i + 1, |e| {
                            matches!(e, Event::End(TagEnd::TableCell))
                        });
                        if in_head {
                            header.push(cell_inlines);
                        } else {
                            current_row.push(cell_inlines);
                        }
                        i = end + 1;
                    }
                    _ => i += 1,
                }
            }
            (
                Some(Block::Table {
                    header,
                    rows,
                    header_source_line,
                    row_source_lines,
                }),
                i - start + 1,
            )
        }
        Tag::HtmlBlock => {
            let mut html = String::new();
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::HtmlBlock) => break,
                    Event::Html(s) | Event::Text(s) => html.push_str(s),
                    _ => {}
                }
                i += 1;
            }
            (Some(Block::Other(html)), i - start + 1)
        }
        // Unmodeled containers: skip to End and emit nothing. The events
        // inside are dropped — anything moss cares about should be modeled
        // explicitly.
        _ => (None, 1),
    }
}

/// Decide whether a paragraph's inlines qualify for promotion to
/// [`Block::Figure`]. Per shape-spec § 1: exactly one [`Inline::Image`]
/// plus any number of whitespace-only [`Inline::Text`] /
/// [`Inline::LineBreak`] siblings. Any other inline shape (Emphasis,
/// Strong, Link, Code, non-whitespace Text, …) disqualifies the
/// paragraph and it stays as [`Block::Paragraph`].
///
/// **Empty-alt guard:** if the matched image has an empty alt (decorative
/// image), the paragraph is NOT promoted. This mirrors production's
/// `transform_events` implicit-figure pass which gates on non-empty alt
/// (a `<figure>` whose caption duplicates a missing alt would be useless
/// for assistive tech and adds visual noise). The empty-alt image stays
/// as `<p><img></p>`, matching the production byte shape for the same
/// input — verified via the parity probe's `other` category on 刘果 CJK
/// fixtures (image-only paragraphs with empty alt).
///
/// On qualification, returns `Ok(Block::Figure { image, caption })` with
/// caption defaulting to the image's alt text (parsed as a single
/// [`Inline::Text`] so the renderer's figcaption emission can escape it
/// uniformly with other inline content).
///
/// On disqualification, returns `Err(original_inlines)` so the caller
/// can fall back to constructing the standard `Block::Paragraph` without
/// re-walking events.
fn try_promote_to_figure(inlines: Vec<Inline>) -> Result<Block, Vec<Inline>> {
    let mut image_count = 0;
    for inline in &inlines {
        match inline {
            Inline::Image { .. } => image_count += 1,
            Inline::Text(s) if s.trim().is_empty() => {} // whitespace OK
            Inline::LineBreak => {}                      // line break OK
            _ => return Err(inlines),
        }
    }
    if image_count != 1 {
        return Err(inlines);
    }

    // Non-image wikilink embeds never promote. pulldown-cmark parses every
    // `![[…]]` as an Image event, but Figure is an image concept: a video /
    // pdf / audio wikilink promoted here bypasses `dispatch_wikilink_embeds`
    // (which only dispatches Paragraph-shaped lone embeds), so its typed
    // synthesizer never runs and the page ships `<figure><img src="clip.mov">`
    // — a broken image. The gate keys off the same classifier the dispatcher
    // uses (`resolve::ext_kind`), so parse-time promotion and dispatch-time
    // synthesis cannot disagree about who owns the block. Extension-less
    // wikilinks (`![[draft|55%]]`) also stay Paragraph: only the with-graph
    // dispatcher can resolve their kind, and committing them to an image
    // Figure here would be a guess.
    if let Some(Inline::Image {
        src,
        is_wikilink: true,
        ..
    }) = inlines.iter().find(|i| matches!(i, Inline::Image { .. }))
    {
        let dest = match src {
            Url::Unresolved(s) => s.as_str(),
            Url::Resolved(r) => r.href.as_str(),
        };
        let ext = crate::path_ext::path_extension_lower(dest);
        if !matches!(
            crate::resolve::ext_kind::reference_kind_for_ext(&ext),
            crate::resolve::ext_kind::ExtKind::Image
        ) {
            return Err(inlines);
        }
    }

    // Probe the width + remaining alt on a BORROW first, so the empty-alt
    // guard can still return `Err(inlines)` with the original whitespace /
    // line-break siblings intact (production `<p><img>…</p>` parity).
    //
    // Standard-markdown images carry no structured pothole — a `|55%`/`|wide`
    // width rides in the raw alt text. Split it out so the figure carries the
    // width and the caption is the remaining alt.
    //
    // Wikilink images carry the raw pothole in `wikilink_pothole`; named width
    // tokens are already classified by `parse_pothole_params` (WidthToken arm),
    // but a content-relative percent (`55%`) is classified as `Alias` and
    // lands in `alt` (or is stripped from alt by our parser-level Alias fix).
    // Recover the percent from `wikilink_pothole` directly so the figure
    // carries the width on both the with-graph path (wikilink_dispatch) and
    // the no-graph path (fragment/test render with no ContentGraph).
    let mut figure_width: Option<String> = None;
    let mut rewritten_alt: Option<String> = None;
    match inlines.iter().find(|i| matches!(i, Inline::Image { .. })) {
        Some(Inline::Image {
            alt,
            is_wikilink: false,
            ..
        }) => {
            let (rest_alt, w) = crate::media::split_alt_width(alt);
            if w.is_some() {
                figure_width = w;
                rewritten_alt = Some(rest_alt);
            }
        }
        Some(Inline::Image {
            is_wikilink: true,
            wikilink_pothole,
            ..
        }) => {
            // Recover a content-relative percent from the raw pothole.
            // Named tokens are already absent from `alt` (WidthToken arm in
            // parse_pothole_params clears them); only the percent case falls
            // through as `Alias` and still needs extracting.
            // Sync: the with-graph twin lives in resolve/wikilink_dispatch.rs
            // (image branch, ~line 565) — both split width via media::split_alt_width.
            if let Some(pothole) = wikilink_pothole {
                let (remaining, w) = crate::media::split_alt_width(pothole);
                if w.is_some() {
                    figure_width = w;
                    // The remaining pothole (caption after stripping the %) is
                    // the intended caption; propagate it as the rewritten alt if
                    // the current alt is empty (percent-only pothole) or already
                    // stripped to the same value.
                    rewritten_alt = Some(remaining);
                }
            }
        }
        _ => {}
    }

    // The figure's caption text is the effective alt (width-stripped if a
    // width was present, else the raw alt), trimmed.
    let raw_alt = inlines.iter().find_map(|i| match i {
        Inline::Image { alt, .. } => Some(alt.as_str()),
        _ => None,
    });
    let alt_text = rewritten_alt
        .as_deref()
        .or(raw_alt)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Empty-alt guard: refuse to promote a decorative image (preserve the
    // original `<p><img></p>` shape with its whitespace siblings) — UNLESS it
    // carries a width, which needs a figure to hold the inline
    // `style="width:NN%"` / `data-width=`.
    if alt_text.is_empty() && figure_width.is_none() {
        return Err(inlines);
    }

    // Extract the single image, applying the width-stripped alt if any.
    let mut image_owned: Option<Inline> = None;
    for inline in inlines.into_iter() {
        if matches!(inline, Inline::Image { .. }) {
            image_owned = Some(inline);
            break;
        }
    }
    let mut image =
        image_owned.expect("invariant: image_count == 1 implies one Image present");
    if let (Some(new_alt), Inline::Image { alt, .. }) = (rewritten_alt, &mut image) {
        *alt = new_alt;
    }

    // Caption defaults to the remaining alt text; empty alt yields None so no
    // empty <figcaption> is emitted.
    let caption = if alt_text.is_empty() {
        None
    } else {
        Some(vec![Inline::Text(alt_text)])
    };

    Ok(Block::Figure {
        image,
        caption,
        width: figure_width,
        align: None,
        class_names: Vec::new(),
        img_style: None,
    })
}

/// Collect a contiguous run of inline events into `Vec<Inline>`. Stops
/// when `is_end(event)` returns true or events run out. Returns the
/// collected inlines and the end-event index.
fn collect_inlines_until<F>(events: &[Event<'_>], start: usize, is_end: F) -> (Vec<Inline>, usize)
where
    F: Fn(&Event<'_>) -> bool,
{
    let mut out: Vec<Inline> = Vec::new();
    let mut i = start;
    while i < events.len() {
        if is_end(&events[i]) {
            return (out, i);
        }
        let (inline, advance) = parse_inline(events, i);
        if let Some(node) = inline {
            out.push(node);
        }
        i += advance.max(1);
    }
    (out, i)
}

/// Parse one inline construct starting at `events[start]`.
fn parse_inline(events: &[Event<'_>], start: usize) -> (Option<Inline>, usize) {
    match &events[start] {
        Event::Text(t) => (Some(Inline::Text(t.to_string())), 1),
        Event::Code(c) => (Some(Inline::Code(c.to_string())), 1),
        // Phase 4 PR4.5 (2026-05-28): match pulldown-cmark's `push_html`
        // byte shape — SoftBreak emits `\n` between inline siblings, not a
        // space. The space form was a long-standing AST quirk surfaced
        // by Grid cells now flowing through the AST renderer; production
        // baselines (chps-site, SoCiviC, snapshot fixtures) preserve the
        // newline (e.g. `Flamboyan Theater · The Clemente\n107 Suffolk
        // Street`). Aligning here closes one row of the parity probe's
        // `whitespace_attribute_order` category.
        Event::SoftBreak => (Some(Inline::Text("\n".to_string())), 1),
        Event::HardBreak => (Some(Inline::LineBreak), 1),
        Event::Html(s) | Event::InlineHtml(s) => (Some(Inline::Other(s.to_string())), 1),
        // Math (ADR-030). Both are LEAF inline events carrying the raw TeX.
        // These arms are load-bearing: without them the two catch-alls below
        // return `(None, 1)` and every equation is silently deleted from the
        // document (`Energy $E = mc^2$.` → `<p>Energy .</p>`).
        //
        // P1 has no typesetting engine, so math renders as its own escaped
        // source — honest, never blank. `Inline::Other` is a RAW passthrough
        // at render time (render.rs), which is exactly why the escaping has
        // to happen HERE, at construction: the TeX is author input and is
        // full of `<`, `>` and `&`. ADR-030 §4 records why this rides
        // `Inline::Other` instead of a new `Inline::Math` variant (the enum
        // is published, serialized and not `#[non_exhaustive]`, so a variant
        // is a semver one-way door).
        Event::InlineMath(tex) => (Some(math_inline(tex, false)), 1),
        Event::DisplayMath(tex) => (Some(math_inline(tex, true)), 1),
        Event::Start(tag) => match tag {
            Tag::Emphasis => {
                let (children, end) = collect_inlines_until(events, start + 1, |e| {
                    matches!(e, Event::End(TagEnd::Emphasis))
                });
                (Some(Inline::Emphasis(children)), end - start + 1)
            }
            Tag::Strong => {
                let (children, end) = collect_inlines_until(events, start + 1, |e| {
                    matches!(e, Event::End(TagEnd::Strong))
                });
                (Some(Inline::Strong(children)), end - start + 1)
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                let (children, end) = collect_inlines_until(events, start + 1, |e| {
                    matches!(e, Event::End(TagEnd::Link))
                });
                let title_opt = if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                };
                // Phase 4 PR7a (2026-05-28): preserve pulldown-cmark's
                // `LinkType::WikiLink` discriminator on the typed AST so
                // the renderer can emit `class="wikilink"` and graph
                // builders can identify wikilink targets.
                let is_wikilink = matches!(*link_type, pulldown_cmark::LinkType::WikiLink { .. });
                (
                    Some(Inline::Link {
                        url: Url::unresolved(dest_url.to_string()),
                        title: title_opt,
                        children,
                        is_wikilink,
                    }),
                    end - start + 1,
                )
            }
            Tag::Image {
                link_type,
                dest_url,
                title,
                ..
            } => {
                // Collect alt text from text events between Start/End.
                let mut alt = String::new();
                let mut i = start + 1;
                while i < events.len() {
                    match &events[i] {
                        Event::End(TagEnd::Image) => break,
                        Event::Text(t) => alt.push_str(t),
                        Event::Code(c) => alt.push_str(c),
                        // `alt` is a plain-text attribute AND (via the
                        // implicit-figure path) the visible `<figcaption>`,
                        // so math is carried as its markdown source, not as
                        // the `<code>` node. Dropping it deleted the
                        // equation from both surfaces.
                        Event::InlineMath(t) => alt.push_str(&math_source(t, false)),
                        Event::DisplayMath(t) => alt.push_str(&math_source(t, true)),
                        _ => {}
                    }
                    i += 1;
                }
                // PR3.5 (2026-05-28): for wikilink images (`![[file]]` /
                // `![[file|pothole]]`), pulldown-cmark synthesizes text
                // events that aren't always author-intended alt:
                //   - `![[logo.png]]` → text "logo.png" (synthesized from
                //     dest); production treats as empty alt.
                //   - `![[logo.png|contain center]]` → text "contain center"
                //     (display-attrs); production classifies as styling,
                //     NOT alt.
                //   - `![[logo.png|width=400]]` → text "width=400" (typed
                //     params); production classifies as params, NOT alt.
                //   - `![[logo.png|My caption]]` → text "My caption";
                //     genuine alt.
                //
                // Without this classification, PR3's Block::Figure
                // detection (Wave 1) promotes wikilink-image paragraphs
                // with synth-derived "alt" to Figure with bogus
                // figcaptions ("logo.png", "contain center"). Match
                // production's transform_events wikilink-dispatch by
                // running the same classifiers (`is_all_display_keywords`
                // + `parse_pothole_params`) here.
                //
                // PR7a-flip-core-B (2026-05-28): preserve the ORIGINAL
                // pothole text on `Inline::Image.wikilink_pothole`
                // BEFORE alt-classification consumes it.
                // `dispatch_wikilink_embeds` needs the raw pothole to
                // route `![[v.mp4|width=400]]` → typed video synth with
                // the `width=400` param intact (alt-classification would
                // erase it). The pothole is the substring after `|`;
                // pulldown-cmark gives us the synthesized text, so we
                // strip the dest synth case (text == dest_url ⇒ no
                // pothole) and otherwise carry the trimmed alt.
                let is_wikilink_image =
                    matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. });
                let wikilink_pothole: Option<String> = if is_wikilink_image {
                    let dest_str: &str = dest_url;
                    let trimmed = alt.trim();
                    if trimmed.is_empty() || trimmed == dest_str {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                } else {
                    None
                };
                if is_wikilink_image {
                    let dest_str: &str = dest_url;
                    let trimmed = alt.trim().to_string();
                    if trimmed.is_empty() || trimmed == dest_str {
                        // Empty pothole OR pulldown-cmark synthesized
                        // dest_url as text → no author alt.
                        alt.clear();
                    } else if crate::media::is_all_display_keywords(&trimmed) {
                        // `contain center`, `left top`, etc. → display
                        // attrs (production maps to style), not alt.
                        alt.clear();
                    } else {
                        use crate::resolve::wikilink_dispatch::{
                            parse_pothole_params, PotholeContent,
                        };
                        match parse_pothole_params(&trimmed) {
                            PotholeContent::Empty | PotholeContent::Params(_) => {
                                alt.clear();
                            }
                            PotholeContent::WidthToken { rest_alias, .. } => {
                                alt = rest_alias;
                            }
                            PotholeContent::Alias(text) => {
                                // `parse_pothole_params` classifies a content-relative
                                // percent (e.g. `55%`) as `Alias` because it is not a
                                // named width token. Intercept it here: a bare percent
                                // is NOT a caption — strip it from the alt so it does
                                // not leak to `<figcaption>`. The actual width is
                                // recovered from `wikilink_pothole` by
                                // `dispatch_wikilink_embeds` (with-graph path) or
                                // directly from `split_alt_width` in the parser's
                                // `try_promote_to_figure` (no-graph path via `alt`).
                                //
                                // `split_alt_width` returns the remaining caption and
                                // the width token. If the whole alias was a width
                                // (nothing remaining), clear alt.
                                let (remaining, _w) = crate::media::split_alt_width(&text);
                                alt = remaining;
                            }
                        }
                    }
                }
                let title_opt = if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                };
                (
                    Some(Inline::Image {
                        src: Url::unresolved(dest_url.to_string()),
                        alt,
                        title: title_opt,
                        is_wikilink: is_wikilink_image,
                        wikilink_pothole,
                    }),
                    i - start + 1,
                )
            }
            // Unmodeled inline container: skip to its End.
            _ => (None, 1),
        },
        // End / unhandled — caller handles.
        _ => (None, 1),
    }
}

/// Collect a contiguous run of block events into `Vec<Block>`. Stops when
/// `is_end(event)` returns true or events run out.
fn collect_blocks_until<F>(
    events: &[Event<'_>],
    start: usize,
    line_ctx: Option<&LineCtx<'_>>,
    is_end: F,
) -> (Vec<Block>, usize)
where
    F: Fn(&Event<'_>) -> bool,
{
    let mut out: Vec<Block> = Vec::new();
    let mut i = start;
    while i < events.len() {
        if is_end(&events[i]) {
            return (out, i);
        }
        let (block, advance) = parse_block(events, i, line_ctx);
        if let Some(b) = block {
            out.push(b);
        }
        i += advance.max(1);
    }
    (out, i)
}

/// Collect the children of a `Tag::Item` until the matching `End(Item)`.
///
/// Pulldown-cmark's **tight-list** mode emits item contents as inline
/// events (Text/Code/SoftBreak/inline-tag Start...) DIRECTLY inside
/// `Tag::Item` without wrapping in `Tag::Paragraph`. The plain
/// [`collect_blocks_until`] dispatcher would route those events through
/// [`parse_block`], which drops stray inlines — yielding empty `<li></li>`.
///
/// This helper preserves both modes:
/// - Inline events accumulate into a synthesized [`Block::Paragraph`] that
///   is flushed when a block-level event (Tag::Paragraph, Tag::List,
///   nested Tag::Item, etc.) appears or at the end of the item.
/// - Block-level events are parsed via [`parse_block_with_tag`] (the
///   standard path).
///
/// The renderer recognises a single-paragraph item shape and emits
/// `<li>...inline...</li>` without an inner `<p>`, matching production's
/// tight-list output byte-for-byte.
fn collect_item_blocks(
    events: &[Event<'_>],
    start: usize,
    line_ctx: Option<&LineCtx<'_>>,
) -> (Vec<Block>, usize) {
    let mut out: Vec<Block> = Vec::new();
    let mut pending_inlines: Vec<Inline> = Vec::new();
    let mut i = start;
    while i < events.len() {
        if matches!(&events[i], Event::End(TagEnd::Item)) {
            flush_pending_paragraph(&mut out, &mut pending_inlines);
            return (out, i);
        }
        if let Some((inline, advance)) = parse_inline_event(events, i) {
            if let Some(node) = inline {
                pending_inlines.push(node);
            }
            i += advance.max(1);
            continue;
        }
        // Block-level event: flush any accumulated inlines, then parse
        // through the standard dispatcher.
        flush_pending_paragraph(&mut out, &mut pending_inlines);
        let (block, advance) = parse_block(events, i, line_ctx);
        if let Some(b) = block {
            out.push(b);
        }
        i += advance.max(1);
    }
    flush_pending_paragraph(&mut out, &mut pending_inlines);
    (out, i)
}

/// Phase 4 PR4: detect a callout marker inside a blockquote and, if
/// found, assemble the entire `Block::Callout` (with body blocks).
///
/// `start` is the event index AFTER `Start(BlockQuote)`. Returns
/// `Some((Block::Callout, end_index))` where `end_index` is the event
/// index of the matching `End(TagEnd::BlockQuote(_))`, so the outer
/// caller can compute the advance. Returns `None` for plain
/// blockquotes (no `[!type]` marker on the first paragraph).
///
/// Detection rule (shape-spec § 1):
/// - The first event must be `Start(Tag::Paragraph)`.
/// - The leading `Event::Text` run (before the first `SoftBreak` or
///   any non-Text inline event) must match `[!<kind>]`, optionally
///   followed by `+` or `-` for foldable callouts, optionally followed
///   by space + inline title.
/// - The kind is canonicalized via [`CalloutKind::from_raw`]; unknown
///   kinds fall back to [`CalloutKind::Note`]. (Diagnostic threading
///   is a Phase 4 followup — `validation::Diagnostic` is scoped to
///   frontmatter validation today.)
///
/// Why detection runs on events (not parsed children): the inline
/// parser collapses `SoftBreak` events into `Inline::Text` (in PR4.5,
/// emitting `"\n"` to match pulldown-cmark's `push_html`), which makes
/// the marker-line vs body-line boundary an embedded `\n` rather than a
/// distinct AST node. Working at the event layer preserves the
/// SoftBreak boundary so we can split "title" (before SoftBreak) from
/// "body" (after SoftBreak) correctly.
fn detect_and_assemble_callout(
    events: &[Event<'_>],
    start: usize,
    line_ctx: Option<&LineCtx<'_>>,
) -> Option<(Block, usize)> {
    if !matches!(events.get(start), Some(Event::Start(Tag::Paragraph))) {
        return None;
    }
    // Coalesce the leading run of `Event::Text` into one logical
    // string. Stops at SoftBreak, HardBreak, any Start/End tag, or
    // any non-Text inline.
    //
    // Math events join the run as their markdown source. `Callout.title`
    // is a `String`, so source text is the only shape it can hold — and
    // breaking here instead would not merely drop the equation, it would
    // TRUNCATE the title at the first `$` and spill the remainder into the
    // callout body (`[!note] Energy $E=mc^2$ explained` → title "Energy ").
    // If a later phase needs a typed title, this is the line that has to
    // become `Vec<Inline>`.
    let mut leading = String::new();
    let mut i = start + 1;
    while let Some(event) = events.get(i) {
        match event {
            Event::Text(t) => {
                leading.push_str(t);
                i += 1;
            }
            Event::InlineMath(t) => {
                leading.push_str(&math_source(t, false));
                i += 1;
            }
            Event::DisplayMath(t) => {
                leading.push_str(&math_source(t, true));
                i += 1;
            }
            _ => break,
        }
    }
    if leading.is_empty() {
        return None;
    }

    let (raw_kind, fold, title, _marker_byte_len) = parse_callout_marker(&leading)?;
    let kind = CalloutKind::from_raw(raw_kind).unwrap_or(CalloutKind::Note);
    let title: Option<String> = title.map(|s| s.to_string()).filter(|s| !s.is_empty());

    // We've consumed the leading Text events. `i` now points at the
    // first non-Text event in the (still-open) marker paragraph.
    //
    // Three shapes from here:
    //   (A) SoftBreak / HardBreak → body lines continue in the same
    //       Paragraph. Skip the break, then collect inlines until
    //       End(Paragraph). Wrap them in a synthetic Block::Paragraph.
    //   (B) End(Paragraph) immediately → marker-only callout (no body
    //       in the marker paragraph). Skip End(Paragraph).
    //   (C) Another inline event (Start(Emphasis), Code, etc.) → the
    //       marker was actually followed by inline markup on the same
    //       line. Currently treated as title continuation — but we
    //       lack a clean event-level coalescer for inline tags, so we
    //       just collect remaining inlines and wrap them as a body
    //       paragraph. The author can use a separator paragraph for
    //       clarity if they want clean title isolation.
    let mut body_blocks: Vec<Block> = Vec::new();
    let body_paragraph_start: Option<usize> = match events.get(i) {
        Some(Event::SoftBreak) | Some(Event::HardBreak) => {
            // Skip the break; collect remaining inlines for the body
            // paragraph.
            Some(i + 1)
        }
        Some(Event::End(TagEnd::Paragraph)) => {
            // Marker was the entire paragraph. Skip past End.
            i += 1;
            None
        }
        _ => {
            // Other inline events directly following the marker —
            // collect them as body paragraph content. (Edge case;
            // see method comment.)
            Some(i)
        }
    };

    if let Some(body_start) = body_paragraph_start {
        // Collect inlines until End(Paragraph) and synthesize a
        // Block::Paragraph for the marker-paragraph body content.
        let (body_inlines, after_para) = collect_inlines_until(events, body_start, |e| {
            matches!(e, Event::End(TagEnd::Paragraph))
        });
        // Skip past End(Paragraph) itself.
        i = after_para + 1;
        // Trim leading whitespace-only Text inlines (e.g. if the
        // line-break Text(" ") leaks through).
        let trimmed_empty = body_inlines.iter().all(|x| match x {
            Inline::Text(t) => t.trim().is_empty(),
            _ => false,
        });
        if !trimmed_empty {
            body_blocks.push(Block::Paragraph(body_inlines));
        }
    }

    // Continue collecting subsequent blocks until End(BlockQuote).
    while let Some(event) = events.get(i) {
        if matches!(event, Event::End(TagEnd::BlockQuote(_))) {
            break;
        }
        let (block, advance) = parse_block(events, i, line_ctx);
        if let Some(b) = block {
            body_blocks.push(b);
        }
        i += advance.max(1);
    }

    // `i` now points at `End(BlockQuote)`. Return total event
    // span: outer caller computes `i - start + 1` (where `start` here
    // is the pre-Start-BlockQuote index in the outer scope; but we
    // were called with `start = outer_start + 1`, so the outer
    // caller's `start` correctly indexes the opening `Start(BlockQuote)`).
    // Per the call shape in `parse_block_with_tag` Tag::BlockQuote arm:
    //   `match detect_and_assemble_callout(events, start + 1)`
    //   `Some((block, body_end)) => (Some(block), body_end - start + 1)`
    // we must return `body_end = i` (the `End(BlockQuote)` index).
    let block = Block::Callout {
        kind,
        fold,
        title,
        children: body_blocks,
    };
    Some((block, i))
}

/// Parse the leading text of a callout-shaped paragraph.
///
/// Accepts text shaped like `[!kind] title text…`, `[!kind]+ title`,
/// `[!kind]-`, etc. Returns:
/// - `raw_kind` — the kind identifier verbatim (lowercased on
///   canonicalization, not here).
/// - `fold` — `Some(Fold::Open)` for `+`, `Some(Fold::Closed)` for `-`,
///   `None` otherwise.
/// - `title` — `Some(title_text)` when text follows the marker (space
///   separator consumed); `None` when the marker is the entire string.
///   Title may be empty (`""`) if author wrote `[!note] ` with trailing
///   whitespace only — caller treats empty as None.
/// - `marker_byte_len` — number of bytes from the start of `text` that
///   constituted the marker + the single separator space (if any). The
///   caller slices `&text[marker_byte_len..]` to recover trailing body
///   text that should stay in the paragraph (multi-line callouts where
///   pulldown-cmark concatenated lines).
fn parse_callout_marker(text: &str) -> Option<(&str, Option<Fold>, Option<&str>, usize)> {
    let after_open = text.strip_prefix("[!")?;
    let close_offset = after_open.find(']')?;
    let raw_kind = &after_open[..close_offset];
    if raw_kind.is_empty() || raw_kind.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    // Offset within `text` immediately after the `]`.
    let after_bracket_offset = 2 + close_offset + 1;
    let rest = &text[after_bracket_offset..];

    let (fold, after_fold_offset) = match rest.chars().next() {
        Some('+') => (Some(Fold::Open), after_bracket_offset + 1),
        Some('-') => (Some(Fold::Closed), after_bracket_offset + 1),
        _ => (None, after_bracket_offset),
    };

    let rest_after_fold = &text[after_fold_offset..];
    let (title, marker_byte_len) = if rest_after_fold.is_empty() {
        // Marker only, no title segment.
        (None, after_fold_offset)
    } else if let Some(remainder) = rest_after_fold.strip_prefix(' ') {
        // ` title text…` — title is everything in this coalesced
        // leading-text string. Pulldown-cmark splits line breaks into
        // SoftBreak inlines, so this Text inline never contains
        // newlines; the title is bounded by the next non-Text inline.
        let title_str = remainder;
        let consumed = after_fold_offset + 1 + remainder.len();
        (Some(title_str), consumed)
    } else {
        // No separator after marker but more text follows (e.g.
        // `[!note]+body` with no space). Treat as no title; keep the
        // text intact.
        (None, after_fold_offset)
    };

    Some((raw_kind, fold, title, marker_byte_len))
}

/// If `events[i]` is an inline-level event, parse it via the existing
/// [`parse_inline`] machinery and return `(inline, advance)`. Returns
/// `None` for block-level events, end tags, or anything the inline
/// dispatcher doesn't own — letting the caller fall back to the block
/// path.
fn parse_inline_event(events: &[Event<'_>], i: usize) -> Option<(Option<Inline>, usize)> {
    match &events[i] {
        Event::Text(_)
        | Event::Code(_)
        | Event::Html(_)
        | Event::InlineHtml(_)
        | Event::SoftBreak
        | Event::HardBreak
        // Math events are inline leaves. This whitelist is the ONLY way
        // they reach `parse_inline` from `collect_item_blocks`, so omitting
        // them here deletes math inside list items, callouts and table cells
        // while paragraph-level math still looks fine — a wiring failure
        // that a mechanism-level test cannot see.
        | Event::InlineMath(_)
        | Event::DisplayMath(_) => Some(parse_inline(events, i)),
        Event::Start(tag) => match tag {
            Tag::Emphasis | Tag::Strong | Tag::Link { .. } | Tag::Image { .. } => {
                Some(parse_inline(events, i))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Drain `pending_inlines` into a [`Block::Paragraph`] appended to `out`,
/// unless it's empty. No-op when there are no pending inlines.
fn flush_pending_paragraph(out: &mut Vec<Block>, pending_inlines: &mut Vec<Inline>) {
    if !pending_inlines.is_empty() {
        out.push(Block::Paragraph(std::mem::take(pending_inlines)));
    }
}

/// Collect the text content of a heading by walking events between
/// `start..end` (exclusive of the matching `Event::End(TagEnd::Heading)`)
/// and concatenating every `Event::Text` and `Event::Code` payload.
///
/// Mirrors production's `transform_events` heading-text collection at
/// `src-tauri/src/build/markdown/pipeline.rs:1784-1795`. Inline HTML
/// (`Event::InlineHtml` / `Event::Html`) is intentionally skipped so that
/// e.g. `# FAREWELL,<br>AND ERASE` yields the slug for
/// `FAREWELL,AND ERASE` (no `<br>` in the slug). Soft/hard breaks are
/// skipped — production only captures Text + Code. Image alt text and
/// link href text are NOT included; the events inside `Tag::Link` /
/// `Tag::Image` are walked transparently and their `Event::Text`
/// payloads (the link/image label) ARE captured, matching production.
///
/// Math is captured as its **markdown source** (`$…$` / `$$…$$`, via
/// [`math_source`]) rather than as bare TeX. That is what makes turning
/// `[site].math` on a non-breaking change for anchors: the slug for
/// `# Euler $e^{i\pi}=-1$ identity` is byte-identical with math off and on,
/// and matches the raw-line slug the wikilink graph computes in
/// `build/scan/scan.rs`. See [`math_source`] for the full argument.
fn collect_heading_text(events: &[Event<'_>], start: usize, end: usize) -> String {
    let mut text = String::new();
    for i in start..end {
        match &events[i] {
            Event::Text(t) => text.push_str(t),
            Event::Code(c) => text.push_str(c),
            Event::InlineMath(t) => text.push_str(&math_source(t, false)),
            Event::DisplayMath(t) => text.push_str(&math_source(t, true)),
            _ => {}
        }
    }
    text
}

/// Post-parse pass: walk every heading in document order (recursively
/// descending into BlockQuote, List items, and Callout children) and
/// disambiguate duplicate IDs by appending `-1`, `-2`, … to the slug.
///
/// Mirrors the `id_counts: HashMap<String, usize>` behavior at
/// `src-tauri/src/build/markdown/pipeline.rs:1798-1805`:
///
/// - First occurrence of slug `foo` keeps id `foo`; counter starts at 1.
/// - Second occurrence becomes `foo-1`; counter becomes 2.
/// - Third occurrence becomes `foo-2`; counter becomes 3.
///
/// Headings whose base slug is `None` (shouldn't happen post-PR2, but
/// safe-guarded) are left untouched.
fn assign_heading_id_suffixes(blocks: &mut [Block]) {
    let mut id_counts: HashMap<String, usize> = HashMap::new();
    assign_heading_id_suffixes_walk(blocks, &mut id_counts);
}

fn assign_heading_id_suffixes_walk(blocks: &mut [Block], id_counts: &mut HashMap<String, usize>) {
    for block in blocks.iter_mut() {
        match block {
            Block::Heading { id, .. } => {
                if let Some(slug) = id {
                    let count_entry = id_counts.entry(slug.clone()).or_insert(0);
                    let count = *count_entry;
                    if count > 0 {
                        *id = Some(format!("{}-{}", slug, count));
                    }
                    *count_entry = count + 1;
                }
            }
            Block::BlockQuote(children) | Block::Callout { children, .. } => {
                assign_heading_id_suffixes_walk(children, id_counts);
            }
            Block::List { items, .. } => {
                for item in items.iter_mut() {
                    assign_heading_id_suffixes_walk(item, id_counts);
                }
            }
            // Tables/CodeBlocks/Shortcodes/Paragraphs/ThematicBreak/Other
            // cannot contain block-level headings — nothing to descend
            // into. Shortcode bodies (Hero overlay, Grid cells) currently
            // carry their content as String (pre-PR4.5); once promoted to
            // Vec<Block>, this walker will need to descend there too.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::{CalloutKind, Fold, Inline};
    use super::*;

    fn first_block(md: &str) -> Block {
        parse(md)
            .blocks
            .into_iter()
            .next()
            .expect("at least one block")
    }

    // -----------------------------------------------------------------
    // Phase 4 PR4: Block::Callout migration + Obsidian alias canonicalization
    // -----------------------------------------------------------------

    #[test]
    fn parses_basic_callout_with_inline_title() {
        match first_block("> [!note] Heads up\n> Body line 1.\n") {
            Block::Callout {
                kind,
                fold,
                title,
                children,
            } => {
                assert_eq!(kind, CalloutKind::Note);
                assert!(fold.is_none(), "non-foldable callout");
                assert_eq!(title.as_deref(), Some("Heads up"));
                assert!(!children.is_empty(), "body should remain");
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn parses_titleless_callout() {
        match first_block("> [!warning]\n> Watch out.\n") {
            Block::Callout {
                kind,
                fold,
                title,
                children,
            } => {
                assert_eq!(kind, CalloutKind::Warning);
                assert!(fold.is_none());
                assert!(title.is_none(), "no inline title");
                assert!(!children.is_empty());
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_alias_tldr_canonicalizes_to_abstract() {
        match first_block("> [!tldr] Short summary\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Abstract),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_alias_hint_canonicalizes_to_tip() {
        match first_block("> [!hint] Pro tip\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Tip),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_alias_important_canonicalizes_to_tip() {
        match first_block("> [!important] Read this\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Tip),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_alias_check_done_canonicalizes_to_success() {
        for alias in &["check", "done"] {
            let md = format!("> [!{alias}] Yes\n> body\n");
            match first_block(&md) {
                Block::Callout { kind, .. } => assert_eq!(
                    kind,
                    CalloutKind::Success,
                    "alias `{alias}` should canonicalize to Success"
                ),
                other => panic!("alias `{alias}` — expected Callout, got {other:?}"),
            }
        }
    }

    #[test]
    fn callout_alias_help_faq_canonicalizes_to_question() {
        for alias in &["help", "faq"] {
            let md = format!("> [!{alias}] question\n> body\n");
            match first_block(&md) {
                Block::Callout { kind, .. } => assert_eq!(
                    kind,
                    CalloutKind::Question,
                    "alias `{alias}` should canonicalize to Question"
                ),
                other => panic!("alias `{alias}` — expected Callout, got {other:?}"),
            }
        }
    }

    #[test]
    fn callout_alias_caution_attention_canonicalizes_to_warning() {
        for alias in &["caution", "attention"] {
            let md = format!("> [!{alias}] careful\n> body\n");
            match first_block(&md) {
                Block::Callout { kind, .. } => assert_eq!(
                    kind,
                    CalloutKind::Warning,
                    "alias `{alias}` should canonicalize to Warning"
                ),
                other => panic!("alias `{alias}` — expected Callout, got {other:?}"),
            }
        }
    }

    #[test]
    fn callout_alias_fail_missing_canonicalizes_to_failure() {
        for alias in &["fail", "missing"] {
            let md = format!("> [!{alias}] oops\n> body\n");
            match first_block(&md) {
                Block::Callout { kind, .. } => assert_eq!(
                    kind,
                    CalloutKind::Failure,
                    "alias `{alias}` should canonicalize to Failure"
                ),
                other => panic!("alias `{alias}` — expected Callout, got {other:?}"),
            }
        }
    }

    #[test]
    fn callout_alias_error_canonicalizes_to_danger() {
        match first_block("> [!error] bad\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Danger),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_alias_cite_canonicalizes_to_quote() {
        match first_block("> [!cite] source\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Quote),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_foldable_open_suffix() {
        match first_block("> [!note]+ Open by default\n> body\n") {
            Block::Callout {
                kind, fold, title, ..
            } => {
                assert_eq!(kind, CalloutKind::Note);
                assert_eq!(fold, Some(Fold::Open));
                assert_eq!(title.as_deref(), Some("Open by default"));
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_foldable_closed_suffix() {
        match first_block("> [!note]- Closed by default\n> body\n") {
            Block::Callout {
                kind, fold, title, ..
            } => {
                assert_eq!(kind, CalloutKind::Note);
                assert_eq!(fold, Some(Fold::Closed));
                assert_eq!(title.as_deref(), Some("Closed by default"));
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_foldable_without_title() {
        match first_block("> [!tip]+\n> body\n") {
            Block::Callout {
                kind, fold, title, ..
            } => {
                assert_eq!(kind, CalloutKind::Tip);
                assert_eq!(fold, Some(Fold::Open));
                assert!(title.is_none());
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_unknown_kind_falls_back_to_note() {
        // Per shape-spec § 1 — unknown kind canonicalizes to Note.
        // Diagnostic emission is a Phase 4 followup (see parser.rs
        // `promote_callout` comment).
        match first_block("> [!unknownkind] body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Note),
            other => panic!("expected Callout (fallback to Note), got {other:?}"),
        }
    }

    #[test]
    fn callout_multi_paragraph_body_preserves_blocks() {
        let md = "> [!info] Multi\n> First paragraph.\n>\n> Second paragraph.\n";
        match first_block(md) {
            Block::Callout {
                kind,
                title,
                children,
                ..
            } => {
                assert_eq!(kind, CalloutKind::Info);
                assert_eq!(title.as_deref(), Some("Multi"));
                // pulldown-cmark emits two paragraphs in the blockquote
                // body when separated by an empty `>` line.
                let para_count = children
                    .iter()
                    .filter(|b| matches!(b, Block::Paragraph(_)))
                    .count();
                assert!(
                    para_count >= 2,
                    "expected at least 2 paragraphs, got {children:?}"
                );
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_nested_inside_callout() {
        // The docs promise nested callouts. After PR4 the outer is
        // Block::Callout containing an inner Block::Callout in its
        // children (no Stage 1 rewrite needed).
        let md = "> [!warning] Outer\n> Outer content.\n>\n> > [!tip] Inner\n> > Inner content.\n";
        match first_block(md) {
            Block::Callout {
                kind: outer_kind,
                children,
                ..
            } => {
                assert_eq!(outer_kind, CalloutKind::Warning);
                let inner = children.iter().find_map(|b| match b {
                    Block::Callout { kind, title, .. } => Some((*kind, title.clone())),
                    _ => None,
                });
                let (inner_kind, inner_title) =
                    inner.expect("inner Block::Callout missing from outer's children");
                assert_eq!(inner_kind, CalloutKind::Tip);
                assert_eq!(inner_title.as_deref(), Some("Inner"));
            }
            other => panic!("expected outer Callout, got {other:?}"),
        }
    }

    #[test]
    fn plain_blockquote_without_marker_stays_blockquote() {
        // Regression: an ordinary blockquote (no `[!type]` marker) must
        // remain Block::BlockQuote — only callout-shaped blockquotes
        // promote.
        match first_block("> Just a quote.\n> More of the quote.\n") {
            Block::BlockQuote(_) => {} // expected
            other => panic!("expected BlockQuote, got {other:?}"),
        }
    }

    #[test]
    fn blockquote_with_text_starting_like_callout_but_unknown_kind_still_promotes() {
        // The marker `[!xyz]` is structurally a callout — we promote
        // and fall back to Note (per shape-spec). The author can fix
        // by removing the bracket prefix if they wanted a plain quote.
        match first_block("> [!xyz] not a real kind\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Note),
            other => panic!("expected Callout fallback, got {other:?}"),
        }
    }

    #[test]
    fn callout_case_insensitive_kind() {
        // Stage 1 was case-insensitive; preserve that contract.
        match first_block("> [!WARNING] Loud\n> body\n") {
            Block::Callout { kind, .. } => assert_eq!(kind, CalloutKind::Warning),
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn callout_pending_alias_canonicalizes_to_todo() {
        // SoCiviC Theatre's voices.md uses `> [!pending]` — carried
        // over from Stage 1 support.
        match first_block("> [!pending] Trailer video\n> Add when ready.\n") {
            Block::Callout { kind, title, .. } => {
                assert_eq!(kind, CalloutKind::Todo);
                assert_eq!(title.as_deref(), Some("Trailer video"));
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_yields_empty_document() {
        let d = parse("");
        assert!(d.blocks.is_empty());
    }

    #[test]
    fn parses_h1_heading() {
        match first_block("# Hello\n") {
            Block::Heading {
                level,
                children,
                id,
            } => {
                assert_eq!(level, 1);
                // Phase 4 PR2: parser populates id with the Obsidian anchor slug.
                assert_eq!(id.as_deref(), Some("hello"));
                assert!(matches!(&children[0], Inline::Text(t) if t == "Hello"));
            }
            other => panic!("expected Heading, got {other:?}"),
        }
    }

    #[test]
    fn parses_h6_heading() {
        match first_block("###### tiny\n") {
            Block::Heading { level, .. } => assert_eq!(level, 6),
            other => panic!("expected Heading, got {other:?}"),
        }
    }

    #[test]
    fn parses_paragraph_with_text() {
        match first_block("hello world\n") {
            Block::Paragraph(children) => {
                // pulldown-cmark may split into multiple Text events; merge.
                let s: String = children
                    .iter()
                    .filter_map(|i| match i {
                        Inline::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(s, "hello world");
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_link_with_unresolved_url() {
        // Critical contract: every URL starts as Unresolved.
        match first_block("[Docs](docs/)\n") {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link {
                    url,
                    title,
                    children,
                    is_wikilink,
                } => {
                    assert!(url.is_unresolved());
                    match url {
                        Url::Unresolved(s) => assert_eq!(s, "docs/"),
                        _ => unreachable!(),
                    }
                    assert!(title.is_none());
                    assert!(!is_wikilink, "standard markdown link is not a wikilink");
                    assert!(matches!(&children[0], Inline::Text(t) if t == "Docs"));
                }
                other => panic!("expected Link, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_link_with_moss_resolved_prefix_unchanged() {
        // The upstream resolve pipeline emits this shape; the parser must
        // preserve it verbatim for the visitor to classify later.
        match first_block("[t](moss-resolved:foo.md)\n") {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link {
                    url: Url::Unresolved(s),
                    ..
                } => assert_eq!(s, "moss-resolved:foo.md"),
                other => panic!("expected unresolved Link, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parser_link_inherits_wikilink_from_pulldown_cmark() {
        // PR7a Decision 2: pulldown-cmark with ENABLE_WIKILINKS emits
        // `Tag::Link { link_type: LinkType::WikiLink, .. }` for `[[target]]`
        // syntax. The typed AST must preserve that discriminator via
        // `Inline::Link::is_wikilink`. After PR7a flips render_document
        // to production, this flag drives the `class="wikilink"` emission
        // on the <a> tag.
        match first_block("[[wikilink-target]]\n") {
            Block::Paragraph(children) => {
                let link = children
                    .iter()
                    .find(|i| matches!(i, Inline::Link { .. }))
                    .expect("expected an Inline::Link from [[…]]");
                match link {
                    Inline::Link { is_wikilink, .. } => {
                        assert!(
                            *is_wikilink,
                            "[[…]] must set is_wikilink: true on the typed AST"
                        );
                    }
                    _ => unreachable!(),
                }
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }

        // Negative case: a standard markdown link is NOT a wikilink.
        match first_block("[text](href)\n") {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { is_wikilink, .. } => {
                    assert!(!is_wikilink, "[](…) must set is_wikilink: false");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn parses_link_with_title() {
        match first_block(r#"[t](u "the title")"#) {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { title, .. } => assert_eq!(title.as_deref(), Some("the title")),
                other => panic!("expected Link, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_image_with_alt() {
        // Phase 4 PR3 (2026-05-27): an image-only paragraph is now
        // promoted to Block::Figure. Inline::Image lives inside the
        // Figure variant; the URL/alt/title contract is unchanged.
        // For image+text (where Block::Paragraph still applies), see
        // `image_with_caption_text_does_not_promote` below.
        match first_block("![cat photo](cat.jpg)\n") {
            Block::Figure { image, caption, .. } => {
                match image {
                    Inline::Image {
                        src, alt, title, ..
                    } => {
                        assert!(src.is_unresolved());
                        assert_eq!(alt, "cat photo");
                        assert!(title.is_none());
                    }
                    other => panic!("expected Image inside Figure, got {other:?}"),
                }
                let cap = caption.expect("caption from alt text");
                assert_eq!(cap.len(), 1);
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn parses_image_inside_paragraph_with_text() {
        // Companion to `parses_image_with_alt`: an image with sibling
        // prose stays as Block::Paragraph (no figure promotion). Holds
        // the parser's image-extraction contract for the non-figure case.
        match first_block("see ![cat photo](cat.jpg) here\n") {
            Block::Paragraph(children) => {
                let img = children
                    .iter()
                    .find(|i| matches!(i, Inline::Image { .. }))
                    .expect("expected Inline::Image among siblings");
                match img {
                    Inline::Image { src, alt, .. } => {
                        assert!(src.is_unresolved());
                        assert_eq!(alt, "cat photo");
                    }
                    _ => unreachable!(),
                }
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_emphasis_and_strong() {
        let para = parse("*em* and **strong**\n")
            .blocks
            .into_iter()
            .next()
            .unwrap();
        match para {
            Block::Paragraph(children) => {
                let has_em = children.iter().any(|i| matches!(i, Inline::Emphasis(_)));
                let has_strong = children.iter().any(|i| matches!(i, Inline::Strong(_)));
                assert!(has_em, "missing Emphasis: {children:?}");
                assert!(has_strong, "missing Strong: {children:?}");
            }
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn parses_inline_code() {
        match first_block("`some code`\n") {
            Block::Paragraph(children) => {
                assert!(matches!(&children[0], Inline::Code(c) if c == "some code"));
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_unordered_list() {
        match first_block("- one\n- two\n") {
            Block::List { ordered, items, .. } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parser_handles_tight_list_items_with_inline_content() {
        // Phase 4 PR0.6 regression — pulldown-cmark's tight-list mode emits
        // inline events (Text/Strong/etc.) directly inside Tag::Item without
        // wrapping in Tag::Paragraph. Previously `parse_block` dropped these
        // stray inlines, producing empty <li></li> instead of the expected
        // <li><strong>bold</strong> text</li>.
        match first_block("- **bold** text\n- another item\n") {
            Block::List { ordered, items, .. } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2, "expected two items, got {items:?}");
                let first_item = &items[0];
                assert_eq!(
                    first_item.len(),
                    1,
                    "tight item should synthesize a single Paragraph, got {first_item:?}"
                );
                match &first_item[0] {
                    Block::Paragraph(inlines) => {
                        let has_strong = inlines.iter().any(|i| matches!(i, Inline::Strong(_)));
                        let has_text = inlines
                            .iter()
                            .any(|i| matches!(i, Inline::Text(t) if t.contains("text")));
                        assert!(
                            has_strong,
                            "expected Inline::Strong inside item, got {inlines:?}"
                        );
                        assert!(has_text, "expected ' text' Inline::Text, got {inlines:?}");
                    }
                    other => panic!("expected Paragraph inside tight item, got {other:?}"),
                }
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn tight_list_items_with_links_preserved() {
        // Mirrors folder-note-site/obsidian/index.md — wikilinks + images
        // inside list items. Today these parse as Inline::Link / Inline::Image;
        // the contract is just that the inline content is NOT dropped.
        match first_block("- [link](url)\n- ![alt](img.jpg)\n") {
            Block::List { items, .. } => {
                assert_eq!(items.len(), 2);
                let first = &items[0];
                assert_eq!(
                    first.len(),
                    1,
                    "expected one Block::Paragraph, got {first:?}"
                );
                match &first[0] {
                    Block::Paragraph(inlines) => {
                        assert!(
                            inlines.iter().any(|i| matches!(i, Inline::Link { .. })),
                            "expected Inline::Link, got {inlines:?}"
                        );
                    }
                    other => panic!("expected Paragraph, got {other:?}"),
                }
                let second = &items[1];
                match &second[0] {
                    Block::Paragraph(inlines) => {
                        assert!(
                            inlines.iter().any(|i| matches!(i, Inline::Image { .. })),
                            "expected Inline::Image, got {inlines:?}"
                        );
                    }
                    other => panic!("expected Paragraph, got {other:?}"),
                }
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn loose_list_items_with_paragraphs_still_work() {
        // Loose-list mode (blank lines between items) emits items as
        // Tag::Paragraph-wrapped blocks. The fix must not break this path.
        let md = "- first item\n\n- second item\n";
        match first_block(md) {
            Block::List { items, .. } => {
                assert_eq!(items.len(), 2);
                for item in &items {
                    assert_eq!(item.len(), 1, "expected one block per item");
                    assert!(
                        matches!(&item[0], Block::Paragraph(_)),
                        "expected Paragraph, got {:?}",
                        item[0]
                    );
                }
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn tight_list_items_with_nested_list_preserve_structure() {
        // - first
        //   - nested
        // The outer item carries inline "first" + a nested Block::List.
        let md = "- first\n  - nested\n";
        match first_block(md) {
            Block::List { items, .. } => {
                assert_eq!(items.len(), 1);
                let outer = &items[0];
                assert!(
                    outer.iter().any(|b| matches!(b, Block::Paragraph(_))),
                    "expected outer item to carry a Paragraph for 'first', got {outer:?}"
                );
                assert!(
                    outer.iter().any(|b| matches!(b, Block::List { .. })),
                    "expected outer item to carry a nested List, got {outer:?}"
                );
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parses_ordered_list() {
        match first_block("1. first\n2. second\n") {
            Block::List { ordered, items, .. } => {
                assert!(ordered);
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parses_fenced_code_block_with_lang() {
        match first_block("```rust\nfn main() {}\n```\n") {
            Block::CodeBlock { lang, value } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert!(value.contains("fn main"));
            }
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn parses_fenced_code_block_without_lang() {
        match first_block("```\nbare\n```\n") {
            Block::CodeBlock { lang, value } => {
                assert!(lang.is_none());
                assert!(value.contains("bare"));
            }
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn code_block_is_not_parsed_as_shortcode() {
        // Adversarial: the literal `:::buttons` inside a fenced code block
        // must NOT be treated as a shortcode. (Phase A's parser doesn't
        // recognize :::buttons at all yet; this test locks the contract.)
        let md = "```\n:::buttons\n[t](u)\n:::\n```\n";
        match first_block(md) {
            Block::CodeBlock { value, .. } => assert!(value.contains(":::buttons")),
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn parses_blockquote() {
        match first_block("> quoted\n") {
            Block::BlockQuote(children) => {
                assert!(!children.is_empty());
            }
            other => panic!("expected BlockQuote, got {other:?}"),
        }
    }

    #[test]
    fn parses_thematic_break() {
        match first_block("---\n") {
            Block::ThematicBreak => {}
            // Pulldown-cmark may emit a thematic break or treat `---` at the
            // start of a doc as a heading underline. Accept either by
            // checking that the parse produces SOMETHING.
            _other => {
                // Test the unambiguous mid-doc case.
                let d = parse("para\n\n---\n\nmore\n");
                let has_break = d.blocks.iter().any(|b| matches!(b, Block::ThematicBreak));
                assert!(
                    has_break,
                    "expected at least one ThematicBreak: {:?}",
                    d.blocks
                );
            }
        }
    }

    #[test]
    fn parses_table() {
        let md = "| h1 | h2 |\n| --- | --- |\n| a | b |\n| c | d |\n";
        match first_block(md) {
            Block::Table { header, rows, .. } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn html_block_passes_through_as_other() {
        match first_block("<div class=\"raw\">hi</div>\n\n") {
            Block::Other(html) => assert!(html.contains("<div")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn parses_multiple_blocks() {
        let d = parse("# T\n\npara\n\n- li\n");
        assert_eq!(d.blocks.len(), 3);
        assert!(matches!(d.blocks[0], Block::Heading { .. }));
        assert!(matches!(d.blocks[1], Block::Paragraph(_)));
        assert!(matches!(d.blocks[2], Block::List { .. }));
    }

    #[test]
    fn frontmatter_only_input_is_handled() {
        // Frontmatter is stripped by upstream code before reaching the
        // parser. If somehow a `---\nfoo:bar\n---` reaches us, the parser
        // must not panic.
        let _ = parse("---\nfoo: bar\n---\n");
    }

    #[test]
    fn link_inside_heading_is_preserved() {
        match first_block("# [t](u)\n") {
            Block::Heading { children, .. } => {
                assert!(matches!(&children[0], Inline::Link { .. }));
            }
            other => panic!("expected Heading, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Phase 4 PR2: heading ID injection
    // -----------------------------------------------------------------

    fn heading_id(md: &str) -> Option<String> {
        let blocks = parse(md).blocks;
        for block in &blocks {
            if let Block::Heading { id, .. } = block {
                return id.clone();
            }
        }
        None
    }

    #[test]
    fn heading_id_simple_phrase() {
        // SoCiviC `## Mission` baseline case.
        assert_eq!(heading_id("## Mission\n"), Some("mission".to_string()));
    }

    #[test]
    fn heading_id_spaces_become_hyphens() {
        assert_eq!(
            heading_id("# Getting Started\n"),
            Some("getting-started".to_string())
        );
    }

    #[test]
    fn heading_id_with_emphasis_uses_text_content() {
        // `*em*` inside a heading: the inner text is `em`, no surrounding
        // chars come from emphasis itself (production captures only Text/Code).
        assert_eq!(
            heading_id("# Hello *world*\n"),
            Some("hello-world".to_string())
        );
    }

    #[test]
    fn heading_id_with_strong_uses_text_content() {
        assert_eq!(
            heading_id("# Bold **stuff**\n"),
            Some("bold-stuff".to_string())
        );
    }

    #[test]
    fn heading_id_with_inline_link_uses_link_text() {
        // `# [Docs](url)` — the link label "Docs" comes through as Event::Text.
        assert_eq!(heading_id("# [Docs](url)\n"), Some("docs".to_string()));
    }

    #[test]
    fn heading_id_with_inline_code_includes_code_payload() {
        // Production captures Event::Code, so `` `fn(x)` `` enters the slug.
        assert_eq!(
            heading_id("# call `fn(x)`\n"),
            Some("call-fn(x)".to_string())
        );
    }

    #[test]
    fn heading_id_with_inline_html_strips_html() {
        // SoCiviC `# FAREWELL,<br>AND ERASE` — the `<br>` is Event::InlineHtml
        // and must NOT appear in the slug. Production's slug for this is
        // derived from "FAREWELL,AND ERASE".
        let id = heading_id("# FAREWELL,<br>AND ERASE\n").expect("heading id");
        // No `<br>` or `br` injected; punctuation preserved (`,`), spaces → `-`.
        assert!(!id.contains("br"), "got: {id}");
        assert_eq!(id, "farewell,and-erase");
    }

    #[test]
    fn heading_id_cjk_preserved() {
        // 刘果's CJK headings exercise Unicode anchor normalization —
        // characters pass through unchanged (lowercase already, no whitespace).
        assert_eq!(heading_id("## 视频\n"), Some("视频".to_string()));
        assert_eq!(heading_id("## 中文标题\n"), Some("中文标题".to_string()));
    }

    #[test]
    fn heading_id_obsidian_strip_chars() {
        // Pipes / brackets / hashes / backslashes / carets are stripped.
        assert_eq!(heading_id("# Note ^ref\n"), Some("note-ref".to_string()));
        assert_eq!(heading_id("# A | B\n"), Some("a-b".to_string()));
    }

    #[test]
    fn duplicate_headings_get_suffixed_ids() {
        // Production behavior: first occurrence keeps slug; second gets `-1`,
        // third gets `-2`. The HashMap in pipeline.rs:1798 is the contract.
        let md = "# Mission\n\n# Mission\n\n# Mission\n";
        let doc = parse(md);
        let ids: Vec<Option<String>> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                Some("mission".to_string()),
                Some("mission-1".to_string()),
                Some("mission-2".to_string()),
            ]
        );
    }

    #[test]
    fn duplicate_suffix_descends_into_blockquote() {
        // Headings inside a blockquote share the same id-counter as top-level.
        let md = "# Notes\n\n> # Notes\n";
        let doc = parse(md);
        let mut found_ids: Vec<String> = Vec::new();
        collect_heading_ids_recursive(&doc.blocks, &mut found_ids);
        assert_eq!(found_ids, vec!["notes".to_string(), "notes-1".to_string()]);
    }

    fn collect_heading_ids_recursive(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Heading { id, .. } => {
                    if let Some(s) = id {
                        out.push(s.clone());
                    }
                }
                Block::BlockQuote(children) | Block::Callout { children, .. } => {
                    collect_heading_ids_recursive(children, out);
                }
                Block::List { items, .. } => {
                    for item in items {
                        collect_heading_ids_recursive(item, out);
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn heading_id_empty_text_yields_empty_slug() {
        // Edge case: `# ###` strips to empty slug; suffix counter still ticks.
        // (obsidian_heading_anchor("") == "")
        let md = "# ###\n";
        let id = heading_id(md);
        assert_eq!(id, Some(String::new()));
    }

    #[test]
    fn link_inside_emphasis_unwraps_correctly() {
        // *[link](u)* — emphasis wrapping a link is a real authoring pattern.
        match first_block("*[t](u)*\n") {
            Block::Paragraph(children) => match &children[0] {
                Inline::Emphasis(inner) => {
                    assert!(matches!(&inner[0], Inline::Link { .. }));
                }
                other => panic!("expected Emphasis, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Phase 4 PR3 (2026-05-27): Block::Figure detection in Tag::Paragraph
    // -----------------------------------------------------------------

    #[test]
    fn image_only_paragraph_promotes_to_figure() {
        // Canonical case: a paragraph containing exactly one image, no
        // sibling inline content, becomes Block::Figure. Caption defaults
        // to the image's alt text.
        match first_block("![A logo](logo.png)\n") {
            Block::Figure { image, caption, .. } => {
                match image {
                    Inline::Image { src, alt, .. } => {
                        assert!(src.is_unresolved());
                        assert_eq!(alt, "A logo");
                    }
                    other => panic!("expected Image inside Figure, got {other:?}"),
                }
                let cap = caption.expect("caption from alt text");
                assert_eq!(cap.len(), 1);
                assert!(matches!(&cap[0], Inline::Text(t) if t == "A logo"));
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn image_only_paragraph_with_empty_alt_stays_as_paragraph() {
        // Empty-alt guard: a decorative image (no alt) does NOT promote
        // to Figure. Production's implicit-figure pass gates on
        // non-empty alt — wrapping a no-alt image in `<figure>` adds
        // visual noise (no figcaption text) without a11y benefit. The
        // bytes match production's `<p><img></p>` shape.
        //
        // Parity-probe evidence: pre-guard, 7 CJK 刘果 fixtures with
        // trailing empty-alt images flipped to "other" because the AST
        // emitted `<figure>` and prod did not. Guard restores parity.
        match first_block("![](logo.png)\n") {
            Block::Paragraph(children) => {
                assert_eq!(children.len(), 1);
                match &children[0] {
                    Inline::Image { alt, .. } => assert_eq!(alt, ""),
                    other => panic!("expected Image inside Paragraph, got {other:?}"),
                }
            }
            other => panic!("empty-alt image-only paragraph must stay as Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn image_with_whitespace_text_still_promotes_to_figure() {
        // Whitespace-only text or line-break siblings don't disqualify
        // (matches transform_events' "image-only modulo whitespace"
        // behavior). Verifying via a wikilink + trailing whitespace would
        // require an actual whitespace event; pulldown-cmark typically
        // strips this. The detector is defensive for the cases that
        // DO surface whitespace inlines (line breaks after the image).
        let md = "![alt](a.jpg)  \n";
        // The trailing "  \n" inside a paragraph emits a HardBreak event
        // (Inline::LineBreak). Promotion must still succeed.
        match first_block(md) {
            Block::Figure { image, .. } => assert!(matches!(image, Inline::Image { .. })),
            // pulldown-cmark may also collapse this differently; accept
            // Paragraph(LineBreak) as a tolerated fallback so the test is
            // not over-specified on pulldown-cmark whitespace semantics.
            // The critical regression we want to lock is that genuine
            // image+text mixes DON'T promote (covered by the test below).
            Block::Paragraph(_) => {}
            other => panic!("expected Figure or Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn image_with_caption_text_does_not_promote() {
        // Critical regression guard (cf. PR1 v2 commit 71c657af3): a
        // paragraph carrying image + prose / emphasis must NOT be
        // promoted to a figure. If we promoted, the caption text would
        // be lost and we'd produce a malformed figure with sibling
        // content swallowed.
        match first_block("![alt](a.jpg) plain caption text\n") {
            Block::Paragraph(children) => {
                assert!(children.iter().any(|i| matches!(i, Inline::Image { .. })));
                assert!(
                    children
                        .iter()
                        .any(|i| matches!(i, Inline::Text(t) if t.contains("plain"))),
                    "expected sibling Text to remain, got {children:?}"
                );
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn image_with_emphasis_sibling_does_not_promote() {
        // Pandoc-style "image + emphasis caption" is recognized in the
        // legacy transform_events as a captioned figure, but PR3's
        // simplified detection (one Image, no other content modulo
        // whitespace) leaves these as Paragraph. PR0's parity probe
        // already classifies these under image_emission / image_figures
        // depending on production behavior; PR3 owns ONLY the simple
        // image-only case. The downstream image+emphasis case is closed
        // out at PR7a when production flips.
        match first_block("![alt](a.jpg) *caption*\n") {
            Block::Paragraph(children) => {
                assert!(children.iter().any(|i| matches!(i, Inline::Image { .. })));
                assert!(
                    children.iter().any(|i| matches!(i, Inline::Emphasis(_))),
                    "expected Emphasis to remain, got {children:?}"
                );
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn two_images_in_one_paragraph_do_not_promote() {
        // Detection rule requires EXACTLY one image. Two images stay as
        // a paragraph (no figure wrap chosen — production would also
        // not wrap this in a figure).
        match first_block("![a](a.jpg) ![b](b.jpg)\n") {
            Block::Paragraph(children) => {
                let img_count = children
                    .iter()
                    .filter(|i| matches!(i, Inline::Image { .. }))
                    .count();
                assert_eq!(img_count, 2);
            }
            other => panic!("expected Paragraph (two images), got {other:?}"),
        }
    }

    // Editor Image UX (2026-06-04): standard-image `|NN%` width carries
    // into Block::Figure.width instead of leaking into the caption.
    // ------------------------------------------------------------------

    #[test]
    fn standard_image_percent_promotes_with_width() {
        // ![alt|55%](pic.jpg) → Figure { width: Some("55%"), caption "alt" }
        match first_block("![alt|55%](pic.jpg)\n") {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                // caption is the remaining alt (width segment removed)
                let cap = caption.expect("caption from remaining alt");
                assert!(matches!(cap.as_slice(), [Inline::Text(t)] if t == "alt"));
            }
            other => panic!("expected a Figure, got {other:?}"),
        }
    }

    #[test]
    fn standard_image_percent_empty_alt_still_promotes() {
        // ![|55%](pic.jpg) → Figure (no caption) carrying the width.
        match first_block("![|55%](pic.jpg)\n") {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                assert!(
                    caption.is_none() || matches!(caption.as_deref(), Some([])),
                    "empty-alt-with-width figure must not carry a caption: {caption:?}"
                );
            }
            other => panic!("expected a Figure even with empty alt when width present, got {other:?}"),
        }
    }

    #[test]
    fn standard_image_no_width_unchanged() {
        // ![alt](pic.jpg) → Figure { width: None } (existing behavior)
        match first_block("![alt](pic.jpg)\n") {
            Block::Figure { width, .. } => assert_eq!(width, None),
            other => panic!("expected a Figure, got {other:?}"),
        }
    }

    #[test]
    fn plain_paragraph_still_parses_as_paragraph() {
        // No regression: a normal text paragraph stays as Block::Paragraph.
        match first_block("just some prose\n") {
            Block::Paragraph(_) => {}
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Phase B Task 7: :::subscribe end-to-end
    // -----------------------------------------------------------------

    use super::super::shortcode::Shortcode;

    #[test]
    fn parses_subscribe_block_into_typed_shortcode() {
        let md = r#":::subscribe {placeholder="you@domain.com" button="Sign me up"}
:::
"#;
        let doc = parse(md);
        // Should find one Block::Shortcode(Subscribe) at top level.
        let mut found: Option<&Shortcode> = None;
        for block in &doc.blocks {
            if let Block::Shortcode(sc) = block {
                found = Some(sc);
                break;
            }
        }
        let sc = found.expect("expected Block::Shortcode");
        match sc {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.placeholder.as_deref(), Some("you@domain.com"));
                assert_eq!(args.button.as_deref(), Some("Sign me up"));
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn subscribe_block_does_not_leave_sentinel_in_other_block() {
        let md = ":::subscribe\n:::\n";
        let doc = parse(md);
        // No Block::Other should contain the sentinel string.
        for block in &doc.blocks {
            if let Block::Other(html) = block {
                assert!(
                    !html.contains("MOSS_SHORTCODE"),
                    "unsubstituted sentinel remained in AST: {html:?}"
                );
            }
        }
    }

    #[test]
    fn subscribe_inside_paragraph_text_is_not_extracted() {
        // Adversarial: `:::subscribe` appearing inside running prose
        // (not as a block opener on its own line) is not a shortcode.
        // The extractor only matches when `:::name` is on its own line.
        let md = "Read more about :::subscribe in the docs.\n";
        let doc = parse(md);
        for block in &doc.blocks {
            assert!(
                !matches!(block, Block::Shortcode(_)),
                "`:::subscribe` inline-text was wrongly extracted as a shortcode"
            );
        }
    }

    #[test]
    fn subscribe_block_alongside_other_content_preserves_order() {
        let md = "# H\n\nfirst para\n\n:::subscribe\ndescription: d\n:::\n\nlast para\n";
        let doc = parse(md);
        let kinds: Vec<&'static str> = doc
            .blocks
            .iter()
            .map(|b| match b {
                Block::Heading { .. } => "h",
                Block::Paragraph(_) => "p",
                Block::Shortcode(_) => "sc",
                _ => "x",
            })
            .collect();
        assert_eq!(kinds, vec!["h", "p", "sc", "p"]);
    }

    // -----------------------------------------------------------------
    // 2026-05-28 (Phase 4 source-line wiring): ParseConfig threading
    // -----------------------------------------------------------------

    #[test]
    fn parse_default_config_keeps_block_meta_empty() {
        let doc = parse("# H1\n\npara one\n\npara two\n");
        assert_eq!(doc.blocks.len(), 3);
        assert_eq!(doc.block_meta.len(), doc.blocks.len());
        for meta in &doc.block_meta {
            assert!(
                meta.source_line.is_none(),
                "default parse should not populate source_line: {meta:?}"
            );
        }
    }

    #[test]
    fn parse_with_source_lines_assigns_1_based_line_numbers() {
        let md = "# H1\n\npara on line 3\n\n## H2 on line 5\n\npara on line 7\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        // Expected blocks: H1, P, H2, P (4 blocks).
        assert_eq!(doc.blocks.len(), 4);
        assert_eq!(doc.block_meta.len(), 4);
        // Line numbers should track the markdown source.
        assert_eq!(doc.block_meta[0].source_line, Some(1), "H1 on line 1");
        assert_eq!(doc.block_meta[1].source_line, Some(3), "P on line 3");
        assert_eq!(doc.block_meta[2].source_line, Some(5), "H2 on line 5");
        assert_eq!(doc.block_meta[3].source_line, Some(7), "P on line 7");
    }

    #[test]
    fn source_line_offset_is_applied_additively() {
        // The parser applies `source_line_offset` additively to every block's
        // body-relative line. What the offset MEANS (how it maps the body back
        // to the editor's CM6 buffer) is decided by the caller in pipeline.rs —
        // this test only pins the additive mechanism, not a coordinate model.
        let md = "# H1\n\npara on line 3\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 7,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        assert_eq!(
            doc.block_meta[0].source_line,
            Some(8),
            "H1 body-line 1 + offset 7"
        );
        assert_eq!(
            doc.block_meta[1].source_line,
            Some(10),
            "P body-line 3 + offset 7"
        );
    }

    #[test]
    fn source_lines_not_collapsed_across_multiline_shortcode() {
        // A multi-line shortcode (grid) must NOT collapse the source lines of
        // blocks after it. The grid spans lines 3–11; the heading after is on
        // line 13. Before the line-count-preserving placeholder fix it
        // collapsed to ~line 5, so editor→preview scroll-sync sent any cursor
        // past the block to the page bottom.
        let md = "# Title\n\n:::grid 3\n[\n![](a.jpg)\n](/x)\n+++\n[\n![](b.jpg)\n](/y)\n:::\n\n## After\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        // Blocks: H1 (line 1), Shortcode grid (line 3), H2 "After" (line 13).
        let last = doc
            .block_meta
            .last()
            .expect("at least one block")
            .source_line;
        assert_eq!(
            last,
            Some(13),
            "heading after a multi-line grid must keep its real line 13, not a collapsed line"
        );
    }

    #[test]
    fn parse_with_source_lines_lists_and_blockquotes() {
        let md = "- item one\n- item two\n\n> quote on line 4\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.block_meta[0].source_line, Some(1), "ul on line 1");
        assert_eq!(doc.block_meta[1].source_line, Some(4), "bq on line 4");
    }

    // -----------------------------------------------------------------
    // 2026-05-28 (Phase 4 source-line followup): per-<li> + per-<tr>
    // line tracking on Block::List and Block::Table.
    // -----------------------------------------------------------------

    #[test]
    fn parse_with_source_lines_populates_item_lines_on_list() {
        // Multi-item list spanning consecutive source lines; the parser
        // must capture the 1-based line of each `Tag::Item` start.
        let md = "- one\n- two\n- three\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::List {
                items,
                item_source_lines,
                ..
            } => {
                assert_eq!(items.len(), 3);
                assert_eq!(
                    item_source_lines.len(),
                    3,
                    "item_source_lines must be parallel to items"
                );
                assert_eq!(item_source_lines[0], Some(1));
                assert_eq!(item_source_lines[1], Some(2));
                assert_eq!(item_source_lines[2], Some(3));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parse_default_config_leaves_item_source_lines_empty() {
        // Production publish builds (default config — `emit_source_lines:
        // false`) must NOT populate `item_source_lines`. The renderer
        // treats empty as "no annotations" so the published HTML is
        // byte-identical to the pre-followup output.
        let doc = parse("- one\n- two\n");
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::List {
                item_source_lines, ..
            } => {
                assert!(
                    item_source_lines.is_empty(),
                    "default config must NOT populate item_source_lines (publish builds): {item_source_lines:?}"
                );
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_source_lines_populates_row_lines_on_table() {
        // Multi-row table: header on line 1, separator on line 2, body
        // rows on lines 3, 4, 5. The parser must capture the 1-based
        // line of each `Tag::TableRow` start.
        let md = "| h1 | h2 |\n| --- | --- |\n| a | b |\n| c | d |\n| e | f |\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::Table {
                rows,
                header_source_line,
                row_source_lines,
                ..
            } => {
                assert_eq!(rows.len(), 3);
                // The header tr anchors at the markdown header row (line 1).
                assert_eq!(*header_source_line, Some(1), "header tr line");
                assert_eq!(
                    row_source_lines.len(),
                    3,
                    "row_source_lines must be parallel to rows"
                );
                assert_eq!(row_source_lines[0], Some(3));
                assert_eq!(row_source_lines[1], Some(4));
                assert_eq!(row_source_lines[2], Some(5));
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn parse_default_config_leaves_row_source_lines_empty() {
        // Production publish builds must not populate table row lines.
        let md = "| h1 | h2 |\n| --- | --- |\n| a | b |\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::Table {
                header_source_line,
                row_source_lines,
                ..
            } => {
                assert!(header_source_line.is_none());
                assert!(row_source_lines.is_empty());
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // 2026-05-28 (Phase 4 followup B): ordered-list explicit start
    // number captured from pulldown-cmark's `Tag::List(Option<u64>)`
    // payload and round-tripped to the renderer as `<ol start="N">`.
    // -----------------------------------------------------------------

    #[test]
    fn parse_ordered_list_start_3_captures_start_number() {
        // `3. foo` should capture `start: Some(3)` so the renderer can
        // emit `<ol start="3">`. CommonMark only honors the first
        // item's number — subsequent items are re-derived.
        let doc = parse("3. foo\n4. bar\n");
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::List {
                ordered,
                start,
                items,
                ..
            } => {
                assert!(ordered, "ordered list");
                assert_eq!(*start, Some(3), "explicit start number captured");
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected ordered List, got {other:?}"),
        }
    }

    #[test]
    fn parse_ordered_list_default_start_collapses_to_none() {
        // pulldown-cmark normalizes `1. foo` to `Tag::List(Some(1))`,
        // but the AST canonicalizes this to `start: None` (semantically
        // identical to `<ol>` without a `start=` attribute, but cleaner).
        let doc = parse("1. foo\n2. bar\n");
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::List { ordered, start, .. } => {
                assert!(ordered);
                assert!(
                    start.is_none(),
                    "implicit start=1 must collapse to None, got {start:?}"
                );
            }
            other => panic!("expected ordered List, got {other:?}"),
        }
    }

    #[test]
    fn parse_unordered_list_has_no_start() {
        // `- foo` is unordered (`Tag::List(None)`). `start` must always
        // be `None` regardless of any subsequent reasoning.
        let doc = parse("- foo\n- bar\n");
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::List { ordered, start, .. } => {
                assert!(!ordered, "unordered list");
                assert!(
                    start.is_none(),
                    "unordered list must have start=None, got {start:?}"
                );
            }
            other => panic!("expected unordered List, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_source_lines_handles_list_after_blank_line_offset() {
        // List items can start past the document start; verify the
        // 1-based numbering tracks the actual source line, not a
        // 0-based index from the list opener.
        let md = "intro paragraph\n\n- item on line 3\n- item on line 4\n";
        let config = ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config(md, &config);
        assert_eq!(doc.blocks.len(), 2);
        match &doc.blocks[1] {
            Block::List {
                item_source_lines, ..
            } => {
                assert_eq!(item_source_lines.len(), 2);
                assert_eq!(item_source_lines[0], Some(3));
                assert_eq!(item_source_lines[1], Some(4));
            }
            other => panic!("expected List as second block, got {other:?}"),
        }
    }

    #[test]
    fn parse_implicit_figure_default_promotes_image_only_paragraph() {
        // Image-only paragraph with non-empty alt → promoted to Block::Figure.
        let doc = parse("![alt](photo.jpg)\n");
        assert_eq!(doc.blocks.len(), 1);
        assert!(
            matches!(doc.blocks[0], Block::Figure { .. }),
            "default config (implicit_figure=true) should promote: got {:?}",
            doc.blocks[0]
        );
    }

    #[test]
    fn parse_implicit_figure_off_leaves_image_paragraph_unpromoted() {
        let config = ParseConfig {
            emit_source_lines: false,
            implicit_figure: false,
            source_line_offset: 0,
            math: false,
        };
        let doc = parse_with_config("![alt](photo.jpg)\n", &config);
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::Paragraph(inlines) => {
                assert!(matches!(inlines[0], Inline::Image { .. }));
            }
            other => panic!("expected Paragraph with image, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // LineLookup unit tests (binary-search prefix-sum line table)
    // -----------------------------------------------------------------

    #[test]
    fn line_lookup_offset_zero_is_line_one() {
        let lookup = LineLookup::build("hello\nworld\n", 0);
        assert_eq!(lookup.line_at(0), 1);
    }

    #[test]
    fn line_lookup_after_first_newline_is_line_two() {
        let lookup = LineLookup::build("hello\nworld\n", 0);
        // Byte 6 is the 'w' of "world", which is on line 2.
        assert_eq!(lookup.line_at(6), 2);
    }

    #[test]
    fn line_lookup_handles_multiline_block_starts() {
        let lookup = LineLookup::build("line1\nline2\nline3\n", 0);
        // First non-newline byte of each line.
        assert_eq!(lookup.line_at(0), 1, "byte 0 → line 1");
        assert_eq!(lookup.line_at(6), 2, "byte 6 → line 2");
        assert_eq!(lookup.line_at(12), 3, "byte 12 → line 3");
    }

    #[test]
    fn line_lookup_empty_source() {
        let lookup = LineLookup::build("", 0);
        assert_eq!(lookup.line_at(0), 1, "empty source still has line 1");
    }

    // -----------------------------------------------------------------
    // Wikilink percent-width — no-ContentGraph path
    //
    // `try_promote_to_figure` recovers a content-relative percent (`|55%`)
    // from `wikilink_pothole` so the no-graph parse path (fragment/test
    // render) carries width in `Block::Figure.width`, not as a spurious
    // caption. These are regression guards for the no-graph fix.
    //
    // Sync: the with-graph twin lives in
    // resolve/wikilink_dispatch.rs (image branch, `split_alt_width` call)
    // — both split width via `media::split_alt_width`.
    // -----------------------------------------------------------------

    #[test]
    fn wikilink_image_percent_no_graph_promotes_with_width() {
        // ![[pic.jpg|55%]] must carry |55% into Figure.width, not leak it
        // into the caption (regression guard for the no-graph fix).
        let block = first_block("![[pic.jpg|55%]]\n");
        match block {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                assert!(caption.is_none(), "percent must not become a caption");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn wikilink_image_percent_with_caption_no_graph() {
        // ![[pic.jpg|My cap|55%]] — width Some("55%"), caption "My cap".
        let block = first_block("![[pic.jpg|My cap|55%]]\n");
        match block {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                let cap = caption.as_ref().expect("caption must be present");
                assert!(
                    matches!(cap.as_slice(), [Inline::Text(t)] if t == "My cap"),
                    "caption should be the non-width segment, got {cap:?}"
                );
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Non-image wikilink embeds must NOT promote to Figure
    //
    // Figure is an image concept. pulldown-cmark parses every `![[…]]`
    // as an Image event, so without a kind gate a video pothole
    // (`![[clip.mov|77%]]`) was hijacked into Block::Figure — and
    // `dispatch_wikilink_embeds` only dispatches Paragraph-shaped lone
    // embeds, so the video synthesizer never ran: the page shipped
    // `<figure><img src="clip.mov">` (broken image). The gate keys off
    // the same classifier the dispatcher uses (`resolve::ext_kind`), so
    // parse-time promotion and dispatch-time synthesis can never
    // disagree about who owns the block.
    // -----------------------------------------------------------------

    #[test]
    fn wikilink_video_percent_stays_paragraph() {
        let block = first_block("![[clip.mov|77%]]\n");
        match block {
            Block::Paragraph(inlines) => assert!(
                matches!(
                    inlines.as_slice(),
                    [Inline::Image {
                        is_wikilink: true,
                        ..
                    }]
                ),
                "paragraph must hold the lone wikilink image, got {inlines:?}"
            ),
            other => panic!("video embed must stay Paragraph for dispatch, got {other:?}"),
        }
    }

    #[test]
    fn wikilink_video_box_sizing_stays_paragraph() {
        // `|640x360` is the documented video sizing alias — it must reach
        // the dispatcher, not become a figcaption.
        let block = first_block("![[clip.mov|640x360]]\n");
        assert!(
            matches!(block, Block::Paragraph(_)),
            "expected Paragraph, got {block:?}"
        );
    }

    #[test]
    fn wikilink_pdf_alias_stays_paragraph() {
        let block = first_block("![[report.pdf|80%]]\n");
        assert!(
            matches!(block, Block::Paragraph(_)),
            "expected Paragraph, got {block:?}"
        );
    }

    #[test]
    fn wikilink_extensionless_stays_paragraph() {
        // `![[draft|55%]]` carries no extension intent — only the
        // with-graph dispatcher can resolve its kind, so the parser must
        // not commit it to an image Figure.
        let block = first_block("![[draft|55%]]\n");
        assert!(
            matches!(block, Block::Paragraph(_)),
            "expected Paragraph, got {block:?}"
        );
    }

    #[test]
    fn wikilink_uppercase_image_ext_still_promotes() {
        // Extension matching is case-insensitive (vault files like
        // `photo.JPG` are common iPhone/camera exports).
        let block = first_block("![[photo.JPG|55%]]\n");
        assert!(
            matches!(block, Block::Figure { .. }),
            "expected Figure, got {block:?}"
        );
    }
}
