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
//! Heading IDs are not assigned by this parser; they're attached by a
//! later pass (the existing `obsidian_heading_anchor` flow). The parser
//! leaves `Block::Heading.id == None`.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::document::Document;
use super::node::{Block, Inline};
use super::url::Url;

/// Parse markdown into a typed [`Document`].
///
/// This is the AST entry point. The input is post-resolve markdown (the
/// upstream resolve pipeline has already rewritten wikilinks into standard
/// markdown links with `moss-resolved:` prefixes).
pub fn parse(markdown: &str) -> Document {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(markdown, options);
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
    Document::from_blocks(blocks)
}

/// Parse one block-level construct starting at `events[start]`. Returns
/// the parsed block (or `None` if `events[start]` was a closing tag /
/// stray event we skip) and how many events to advance.
fn parse_block(events: &[Event<'_>], start: usize) -> (Option<Block>, usize) {
    match &events[start] {
        Event::Start(tag) => parse_block_with_tag(events, start, tag),
        // Stray Text/Code/etc at top level: wrap in a paragraph.
        Event::Text(_) | Event::Code(_) | Event::Html(_) | Event::SoftBreak | Event::HardBreak => {
            // pulldown-cmark wraps these inside paragraphs already, so this
            // branch is rare. Skip rather than synthesize a paragraph.
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
            (
                Some(Block::Heading {
                    level: level_num,
                    children,
                    id: None,
                }),
                end - start + 1,
            )
        }
        Tag::Paragraph => {
            let (children, end) = collect_inlines_until(events, start + 1, |e| {
                matches!(e, Event::End(TagEnd::Paragraph))
            });
            (Some(Block::Paragraph(children)), end - start + 1)
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
                        let (item_blocks, end) = collect_blocks_until(events, i + 1, |e| {
                            matches!(e, Event::End(TagEnd::Item))
                        });
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
                assert!(id.is_none());
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
        match first_block("![cat photo](cat.jpg)\n") {
            Block::Paragraph(children) => match &children[0] {
                Inline::Image { src, alt, title } => {
                    assert!(src.is_unresolved());
                    assert_eq!(alt, "cat photo");
                    assert!(title.is_none());
                }
                other => panic!("expected Image, got {other:?}"),
            },
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
}
