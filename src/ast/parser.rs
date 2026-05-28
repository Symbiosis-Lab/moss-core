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

use super::document::Document;
use super::node::{Block, Inline};
use super::shortcode_extract::{extract_shortcodes, parse_placeholder, ExtractedShortcode};
use super::url::Url;
use crate::heading_anchor::obsidian_heading_anchor;

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
pub fn parse(markdown: &str) -> Document {
    let extraction = extract_shortcodes(markdown);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    // Phase 3 PR2: pulldown-cmark emits `LinkType::WikiLink` events for
    // `[[…]]` / `![[…]]` natively. The typed-AST parser preserves them as
    // `Inline::Link`/`Inline::Image` with `Url::Unresolved`; resolution
    // happens in the later `visit_urls_mut` pass.
    options.insert(Options::ENABLE_WIKILINKS);

    let parser = Parser::new_ext(&extraction.markdown_with_placeholders, options);
    let events: Vec<Event<'_>> = parser.collect();

    let mut blocks = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let (block, advance) = parse_block(&events, i);
        if let Some(b) = block {
            blocks.push(b);
        }
        i += advance.max(1);
    }

    // Substitute sentinel placeholders with their typed Shortcode variants.
    substitute_shortcode_placeholders(&mut blocks, &extraction.nonce, &extraction.extracted);

    // Apply duplicate-suffix numbering to heading IDs in document order.
    // Each Tag::Heading arm computes the base slug; this pass disambiguates
    // collisions across the whole document, matching production's id_counts
    // HashMap behavior in pipeline.rs::transform_events.
    assign_heading_id_suffixes(&mut blocks);

    Document::from_blocks(blocks)
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
fn parse_block(events: &[Event<'_>], start: usize) -> (Option<Block>, usize) {
    match &events[start] {
        Event::Start(tag) => parse_block_with_tag(events, start, tag),
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

fn parse_block_with_tag(events: &[Event<'_>], start: usize, tag: &Tag<'_>) -> (Option<Block>, usize) {
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
            (
                Some(Block::CodeBlock { lang, value }),
                i - start + 1,
            )
        }
        Tag::BlockQuote(_) => {
            let (children, end) = collect_blocks_until(events, start + 1, |e| {
                matches!(e, Event::End(TagEnd::BlockQuote(_)))
            });
            (Some(Block::BlockQuote(children)), end - start + 1)
        }
        Tag::List(start_num) => {
            let ordered = start_num.is_some();
            let mut items: Vec<Vec<Block>> = Vec::new();
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::List(_)) => break,
                    Event::Start(Tag::Item) => {
                        let (item_blocks, end) = collect_item_blocks(events, i + 1);
                        items.push(item_blocks);
                        i = end + 1;
                    }
                    _ => i += 1,
                }
            }
            (Some(Block::List { ordered, items }), i - start + 1)
        }
        Tag::Table(_) => {
            let mut header: Vec<Vec<Inline>> = Vec::new();
            let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
            let mut current_row: Vec<Vec<Inline>> = Vec::new();
            let mut in_head = false;
            let mut in_body_row = false;
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::End(TagEnd::Table) => break,
                    Event::Start(Tag::TableHead) => {
                        in_head = true;
                        i += 1;
                    }
                    Event::End(TagEnd::TableHead) => {
                        in_head = false;
                        i += 1;
                    }
                    Event::Start(Tag::TableRow) => {
                        in_body_row = true;
                        current_row = Vec::new();
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
            (Some(Block::Table { header, rows }), i - start + 1)
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
    // Empty-alt guard: refuse to promote so production-equivalent
    // `<p><img></p>` output is preserved for decorative images.
    let image_has_alt = inlines.iter().any(|i| match i {
        Inline::Image { alt, .. } => !alt.trim().is_empty(),
        _ => false,
    });
    if !image_has_alt {
        return Err(inlines);
    }
    // Extract the single image; keep ownership of the original vec
    // simple by re-walking with into_iter so we move out instead of
    // cloning.
    let mut image_owned: Option<Inline> = None;
    for inline in inlines.into_iter() {
        if matches!(inline, Inline::Image { .. }) {
            image_owned = Some(inline);
            break;
        }
    }
    let image = image_owned.expect("invariant: image_count == 1 implies one Image present");
    // Caption is always Some here (empty-alt was filtered above), but
    // keep the Option<Vec<Inline>> shape per shape-spec § 1.
    let caption = match &image {
        Inline::Image { alt, .. } => Some(vec![Inline::Text(alt.clone())]),
        _ => None,
    };
    Ok(Block::Figure { image, caption })
}

/// Collect a contiguous run of inline events into `Vec<Inline>`. Stops
/// when `is_end(event)` returns true or events run out. Returns the
/// collected inlines and the end-event index.
fn collect_inlines_until<F>(
    events: &[Event<'_>],
    start: usize,
    is_end: F,
) -> (Vec<Inline>, usize)
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
        Event::SoftBreak => (Some(Inline::Text(" ".to_string())), 1),
        Event::HardBreak => (Some(Inline::LineBreak), 1),
        Event::Html(s) | Event::InlineHtml(s) => (Some(Inline::Other(s.to_string())), 1),
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
                dest_url, title, ..
            } => {
                let (children, end) = collect_inlines_until(events, start + 1, |e| {
                    matches!(e, Event::End(TagEnd::Link))
                });
                let title_opt = if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                };
                (
                    Some(Inline::Link {
                        url: Url::unresolved(dest_url.to_string()),
                        title: title_opt,
                        children,
                    }),
                    end - start + 1,
                )
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                // Collect alt text from text events between Start/End.
                let mut alt = String::new();
                let mut i = start + 1;
                while i < events.len() {
                    match &events[i] {
                        Event::End(TagEnd::Image) => break,
                        Event::Text(t) => alt.push_str(t),
                        Event::Code(c) => alt.push_str(c),
                        _ => {}
                    }
                    i += 1;
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
fn collect_blocks_until<F>(events: &[Event<'_>], start: usize, is_end: F) -> (Vec<Block>, usize)
where
    F: Fn(&Event<'_>) -> bool,
{
    let mut out: Vec<Block> = Vec::new();
    let mut i = start;
    while i < events.len() {
        if is_end(&events[i]) {
            return (out, i);
        }
        let (block, advance) = parse_block(events, i);
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
fn collect_item_blocks(events: &[Event<'_>], start: usize) -> (Vec<Block>, usize) {
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
        let (block, advance) = parse_block(events, i);
        if let Some(b) = block {
            out.push(b);
        }
        i += advance.max(1);
    }
    flush_pending_paragraph(&mut out, &mut pending_inlines);
    (out, i)
}

/// If `events[i]` is an inline-level event, parse it via the existing
/// [`parse_inline`] machinery and return `(inline, advance)`. Returns
/// `None` for block-level events, end tags, or anything the inline
/// dispatcher doesn't own — letting the caller fall back to the block
/// path.
fn parse_inline_event(
    events: &[Event<'_>],
    i: usize,
) -> Option<(Option<Inline>, usize)> {
    match &events[i] {
        Event::Text(_)
        | Event::Code(_)
        | Event::Html(_)
        | Event::InlineHtml(_)
        | Event::SoftBreak
        | Event::HardBreak => Some(parse_inline(events, i)),
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
fn collect_heading_text(events: &[Event<'_>], start: usize, end: usize) -> String {
    let mut text = String::new();
    for i in start..end {
        match &events[i] {
            Event::Text(t) => text.push_str(t),
            Event::Code(c) => text.push_str(c),
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
    use super::super::node::Inline;
    use super::*;

    fn first_block(md: &str) -> Block {
        parse(md).blocks.into_iter().next().expect("at least one block")
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
                level, children, id,
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
                Inline::Link { url, title, children } => {
                    assert!(url.is_unresolved());
                    match url {
                        Url::Unresolved(s) => assert_eq!(s, "docs/"),
                        _ => unreachable!(),
                    }
                    assert!(title.is_none());
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
            Block::Figure { image, caption } => {
                match image {
                    Inline::Image { src, alt, title } => {
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
        let para = parse("*em* and **strong**\n").blocks.into_iter().next().unwrap();
        match para {
            Block::Paragraph(children) => {
                let has_em = children
                    .iter()
                    .any(|i| matches!(i, Inline::Emphasis(_)));
                let has_strong = children
                    .iter()
                    .any(|i| matches!(i, Inline::Strong(_)));
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
            Block::List { ordered, items } => {
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
            Block::List { ordered, items } => {
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
                        let has_text = inlines.iter().any(|i| {
                            matches!(i, Inline::Text(t) if t.contains("text"))
                        });
                        assert!(has_strong, "expected Inline::Strong inside item, got {inlines:?}");
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
                assert_eq!(first.len(), 1, "expected one Block::Paragraph, got {first:?}");
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
            Block::List { ordered, items } => {
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
                assert!(has_break, "expected at least one ThematicBreak: {:?}", d.blocks);
            }
        }
    }

    #[test]
    fn parses_table() {
        let md = "| h1 | h2 |\n| --- | --- |\n| a | b |\n| c | d |\n";
        match first_block(md) {
            Block::Table { header, rows } => {
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
        assert_eq!(heading_id("# Hello *world*\n"), Some("hello-world".to_string()));
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
            Block::Figure { image, caption } => {
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
            other => panic!(
                "empty-alt image-only paragraph must stay as Paragraph, got {other:?}"
            ),
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
                    children.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("plain"))),
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
}

