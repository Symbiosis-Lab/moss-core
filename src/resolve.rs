//! Centralized link resolution — ALL wikilink handling (body AND frontmatter) happens here.
//!
//! This module provides shared types for the resolve phase of the
//! build pipeline, a fuzzy path resolver that wraps
//! [`ContentGraph::resolve_path`](crate::content_graph::ContentGraph::resolve_path),
//! and the top-level [`resolve_content`] function that ties all phases together.
//!
//! **Architectural boundary:** Downstream code (markdown.rs, render.rs) receives
//! already-resolved paths. Do NOT add wikilink parsing or resolution elsewhere.

use crate::asset_snapshot::AssetSnapshot;
use crate::content_graph::ContentGraph;

pub mod block_refs;
pub mod callouts;
pub mod embed_renderer;
pub mod embeds;
pub mod fuzzy_path;
pub mod markdown_links;
pub mod markdown_refs;
pub mod registry;
pub mod title_params;
pub mod wikilink_dispatch;

/// A link going out from a document.
#[derive(Debug, Clone)]
pub struct OutgoingLink {
    pub target_path: String,
    pub display_text: String,
    pub link_type: LinkType,
}

/// The kind of link syntax used.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkType {
    /// `[[target]]` or `[[target|display]]`
    Wikilink,
    /// `![[target]]` — an embedded/transcluded reference
    Embed,
    /// Standard markdown `[text](url)`
    Standard,
}

/// A diagnostic message from the resolve phase.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub source_path: String,
    pub reference: String,
}

/// Result of resolving all Obsidian syntax in a markdown file.
#[derive(Debug)]
pub struct ResolveResult {
    /// Clean markdown with all Obsidian syntax resolved.
    pub content_markdown: String,
    /// All outgoing links from this document.
    pub outgoing_links: Vec<OutgoingLink>,
    /// Warnings and errors encountered during resolution.
    pub diagnostics: Vec<Diagnostic>,
    /// Block IDs extracted from this document.
    pub block_ids: Vec<String>,
    /// (target_path, source_path) pairs for embed dependency tracking.
    pub embed_deps: Vec<(String, String)>,
}

/// Resolve all Obsidian syntax in a markdown file, producing clean standard markdown.
///
/// Pipeline order:
/// 1. Separate frontmatter from body
/// 2. Resolve wikilinks (first pass) -- standard `[[…]]` and `![[…]]` to markdown links / embed markers
/// 3. Resolve embed placeholders -- inline `<!-- moss-embed:… -->` markers with file content
/// 4. Resolve wikilinks (second pass) -- catch wikilinks introduced by embedded content
/// 4.5. Resolve bare filenames in standard markdown images -- `![](photo.jpg)` to resolved paths
/// 4.6. Resolve standard markdown links -- `[text](target.md)` to resolved paths
/// 5. Transform block references -- `^id` markers to HTML anchors
/// 6. Transform callouts -- `> [!type]` to HTML divs
/// 7. Rejoin frontmatter + resolved body
pub fn resolve_content(
    source_path: &str,
    raw_markdown: &str,
    graph: &ContentGraph,
    file_reader: &dyn Fn(&str) -> Option<String>,
) -> ResolveResult {
    let handlers = embeds::MarkerHandlers::new();
    let registry = registry::RendererRegistry::builtin().build();
    resolve_content_with_handlers(
        source_path,
        raw_markdown,
        graph,
        file_reader,
        &registry,
        &handlers,
    )
}

/// Variant of [`resolve_content`] that threads a custom [`registry::RendererRegistry`]
/// (plugin-aware renderer dispatch) and [`embeds::MarkerHandlers`] (resolvers for
/// Deferred markers: notebook, table, plugin renderers) through the pipeline.
///
/// Built-in-only pipelines should call [`resolve_content`]. Pipelines that load
/// plugins at init time build a registry + handlers once and call this variant.
///
/// The handler registry fires in a **new step 4.25** that runs after embed
/// resolution and before the second wikilink pass. This ordering lets
/// Deferred handlers splice content that may itself contain wikilinks.
pub fn resolve_content_with_handlers(
    source_path: &str,
    raw_markdown: &str,
    graph: &ContentGraph,
    file_reader: &dyn Fn(&str) -> Option<String>,
    registry: &registry::RendererRegistry,
    handlers: &embeds::MarkerHandlers<'_>,
) -> ResolveResult {
    // Default-empty snapshot for callers that don't yet thread asset data.
    // Phase 0 Task F1: the snapshot-aware variant exists below and is the
    // entry point production code should migrate to as Phase 1 lights up
    // consumption.
    let empty_snapshot = AssetSnapshot::new();
    resolve_content_with_handlers_and_snapshot(
        source_path,
        raw_markdown,
        graph,
        file_reader,
        registry,
        handlers,
        &empty_snapshot,
    )
}

/// Variant of [`resolve_content_with_handlers`] that additionally threads
/// an [`AssetSnapshot`] through the resolve pipeline.
///
/// **Phase 0**: the snapshot is threaded but **not yet consumed** by any
/// resolver — Stage 1 still emits markdown without reading variants/dims.
/// Phase 1 wires the consumption side in moss-core's synthesizer. The
/// signature exists now so src-tauri's build pipeline can populate the
/// snapshot (from `MediaDimensionLookup` + `AssetRegistry`) and prove the
/// threading path before consumers depend on it.
///
/// See `docs/plans/2026-05-25-phase0-asset-snapshot-and-translator.md`
/// § Phase F for the thread-first / consume-later rationale.
pub fn resolve_content_with_handlers_and_snapshot(
    source_path: &str,
    raw_markdown: &str,
    graph: &ContentGraph,
    file_reader: &dyn Fn(&str) -> Option<String>,
    registry: &registry::RendererRegistry,
    handlers: &embeds::MarkerHandlers<'_>,
    // Phase 0: threaded but not yet consumed. Phase 1 wires up reads.
    _assets: &AssetSnapshot,
) -> ResolveResult {
    // Step 1: Separate frontmatter from body.
    let (frontmatter, body) = split_frontmatter(raw_markdown);

    // Phase 3 PR2: Stage 1's wikilink rewriter + stage1_sweep retire.
    // pulldown-cmark now parses `[[…]]` / `![[…]]` natively via
    // `Options::ENABLE_WIKILINKS` (flipped in PR2 at every Parser::new_ext
    // site), and `transform_events::dispatch_wikilink_at` routes each event
    // through the EmbedRenderer registry. The `stage1_sweep`
    // (`![alt](file.pdf)` → `moss:kind=pdf` title rewrite) is retired per
    // plan Option A: authors who want non-image embeds use the wikilink
    // form `![[report.pdf]]`. See plan v2 § PR2.
    let outgoing_links: Vec<OutgoingLink> = Vec::new();
    let diagnostics: Vec<Diagnostic> = Vec::new();
    let _ = registry; // Phase 3 PR2: registry flows directly to src-tauri's
                      // `transform_events` via `process_markdown_file`; this
                      // crate-side path no longer dispatches embeds in Stage 1.

    // Phase 3 PR2: pre-pass that lowers block-level wikilinks into
    // marker comments BEFORE pulldown-cmark sees them. Two classes of
    // wikilink need this treatment because their output is block-level
    // HTML, and pulldown-cmark wraps single-image paragraphs in `<p>`
    // unconditionally:
    //   - **markdown transclusions** (`![[note]]`, `![[note.md]]`,
    //     `![[note#section]]`) → `<!-- moss-embed:TARGET -->` for
    //     `embeds::resolve_embeds` to inline the body.
    //   - **folder-list embeds** (`![[/dir/|limit:N]]`) →
    //     `<!-- MOSS_MARKER_FOLDER_LIST:… -->` for src-tauri's marker
    //     handlers to expand into card grids.
    // Both cases used to be emitted by Stage 1's wikilink resolver; with
    // that resolver retired, pulldown-cmark's Stage 2 dispatcher would
    // emit the markers inside `<p>` (paragraph context), and
    // `resolve_embeds` would never see them (it scans markdown lines,
    // not rendered HTML). Pre-converting both shapes here mirrors the
    // pre-Phase-3 layering.
    let body = lower_transclusion_and_folder_wikilinks(body, graph, source_path);

    // Step 3: Resolve markdown transclusion embeds. The inlined body of
    // each embedded `.md` file is appended verbatim — its wikilinks (if
    // any) survive into the markdown handed back to src-tauri, where
    // pulldown-cmark + Stage 2 dispatcher resolves them along with the
    // host page's own wikilinks.
    let embed_result = embeds::resolve_embeds(&body, source_path, file_reader);
    let mut diagnostics = diagnostics;
    diagnostics.extend(embed_result.diagnostics);
    let embed_deps = embed_result.embed_deps;

    // Step 3.5: Resolve Deferred markers (notebook, table, plugins). All
    // built-in handlers emit pure HTML (`<iframe>`, `<table>`); plugin
    // handlers must do the same (no raw `[[…]]` in handler output).
    // Skipped cheaply if handlers is empty.
    let deferred_result = embeds::resolve_deferred_markers(&embed_result.content, handlers);
    diagnostics.extend(deferred_result.diagnostics);

    // Step 4.5: Resolve bare filenames in standard markdown images.
    let md_ref_result =
        markdown_refs::resolve_markdown_refs(&deferred_result.content, graph, source_path);
    let mut outgoing_links = outgoing_links;
    outgoing_links.extend(md_ref_result.outgoing_links);
    diagnostics.extend(md_ref_result.diagnostics);

    // Step 4.6: Resolve standard markdown link targets via ContentGraph.
    let md_link_result =
        markdown_links::resolve_markdown_links(&md_ref_result.content, graph, source_path);
    outgoing_links.extend(md_link_result.outgoing_links);
    diagnostics.extend(md_link_result.diagnostics);

    // Step 5: Transform block references.
    let (block_result, block_ids) = block_refs::transform_block_refs(&md_link_result.content);

    // Step 6: Transform callouts.
    //
    // Phase 4 PR4 (2026-05-27) added a typed `Block::Callout` to the
    // moss-core AST (`crates/moss-core/src/ast/parser.rs`'s
    // `Tag::BlockQuote` arm) with proper Obsidian-alias canonicalization
    // and foldable-suffix support. The AST renderer (`render_document`)
    // emits the same canonical callout HTML this Stage 1 pass produces.
    //
    // We keep this Stage 1 pass UNTIL PR7a flips production rendering to
    // `render_document`. Removing Stage 1 before then would regress
    // production output (`> [!note]` would render as plain `<blockquote>`
    // because pulldown-cmark + `html::push_html` don't know callout
    // syntax). PR7a deletes this call + the `callouts` module + the
    // `pub mod callouts;` declaration above + regenerates the
    // callouts-site snapshot fixtures atomically with the production
    // flip. See `docs/plans/2026-05-27-phase4-typed-ast-completion.md`
    // § PR7a for the coordinated removal.
    let callout_result = callouts::transform_callouts(&block_result);

    // Step 7: Resolve frontmatter wikilinks + rejoin with resolved body.
    let content_markdown = match frontmatter {
        Some(fm) => {
            let resolved_fm = resolve_frontmatter_wikilinks(fm, graph, source_path);
            diagnostics.extend(resolved_fm.diagnostics);
            format!("{}{}", resolved_fm.content, callout_result)
        }
        None => callout_result,
    };

    ResolveResult {
        content_markdown,
        outgoing_links,
        diagnostics,
        block_ids,
        embed_deps,
    }
}

/// Phase 3 PR2: lower wikilink-form markdown transclusions
/// (`![[note]]` / `![[note.md]]` / `![[note#section]]`) into the
/// `<!-- moss-embed:TARGET -->` marker shape that
/// [`embeds::resolve_embeds`] consumes. Pure text rewrite — no I/O.
///
/// Why this pre-pass exists: pre-Phase-3, Stage 1's wikilink resolver
/// did this conversion. Phase 3 retires that resolver and routes most
/// wikilink handling through pulldown-cmark's Stage 2 dispatcher in
/// `src-tauri/src/build/markdown/pipeline.rs::transform_events`. But
/// `embeds::resolve_embeds` runs BEFORE pulldown-cmark, so the
/// dispatcher cannot emit the marker in time. We pre-convert the
/// transclusion wikilinks here.
///
/// Only `.md`-extension wikilinks (and extension-less wikilinks
/// resolving to `.md` files) are rewritten. Image / pdf / iframe /
/// video / audio / 3d / notebook / table embeds still flow through the
/// Stage 2 dispatcher untouched.
///
/// The conversion is line-based and respects fenced code blocks
/// (`{```/~~~}` blocks pass through unchanged). It does not honor
/// inline-code spans on a line — wikilinks inside `` `like this` ``
/// would also be rewritten — which mirrors the pre-Phase-3 wikilink
/// resolver's coarse line-level scan.
fn lower_transclusion_and_folder_wikilinks(
    body: &str,
    graph: &ContentGraph,
    source_path: &str,
) -> String {
    let mut output_lines: Vec<String> = Vec::with_capacity(body.lines().count() + 1);
    let mut fence_char: Option<char> = None;
    for line in body.lines() {
        // Fenced code block tracking — same logic as markdown_refs.
        if let Some(fc) = fence_char {
            let trimmed = line.trim_start();
            let closes = trimmed.starts_with(fc)
                && trimmed.chars().take(3).all(|c| c == fc)
                && trimmed.trim_matches(fc).trim().is_empty();
            if closes {
                fence_char = None;
            }
            output_lines.push(line.to_string());
            continue;
        }
        let trimmed = line.trim_start();
        let fence_rest = trimmed
            .strip_prefix("```")
            .map(|r| ('`', r))
            .or_else(|| trimmed.strip_prefix("~~~").map(|r| ('~', r)));
        if let Some((candidate_char, rest)) = fence_rest {
            if !rest.contains(candidate_char) {
                fence_char = Some(candidate_char);
                output_lines.push(line.to_string());
                continue;
            }
        }

        // Rewrite `![[…]]` wikilinks where the resolved target is a
        // markdown file. Single-occurrence per line is the common case;
        // a loop handles multi-occurrence safely.
        let mut rewritten = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(start) = rest.find("![[") {
            rewritten.push_str(&rest[..start]);
            let after = &rest[start + 3..];
            let Some(end) = after.find("]]") else {
                rewritten.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let inner = &after[..end];
            // Pothole-aware: pre-Phase-3 dropped pothole text for the
            // marker (params live in the marker's heading-anchor /
            // query suffix). Today the marker only cares about the
            // `file#section` shape.
            let inner_no_pothole = match inner.split_once('|') {
                Some((f, _)) => f,
                None => inner,
            };
            let (file_part, anchor) = match inner_no_pothole.find('#') {
                Some(p) => (&inner_no_pothole[..p], Some(&inner_no_pothole[p + 1..])),
                None => (inner_no_pothole, None),
            };

            // Skip empty target (`![[]]` is meaningless).
            if file_part.is_empty() {
                rewritten.push_str(&rest[start..start + 3 + end + 2]);
                rest = &rest[start + 3 + end + 2..];
                continue;
            }

            // Folder-list embed: trailing slash dispatches to the
            // `MOSS_MARKER_FOLDER_LIST` marker that src-tauri's marker
            // handler resolves into a card grid. The pothole carries
            // params (limit:N, more, sort:axis) in pipe-encoded form.
            if file_part.ends_with('/') {
                let pothole_raw = match inner.split_once('|') {
                    Some((_, params)) => params,
                    None => "",
                };
                let params = embed_renderer::folder_list::parse_params(pothole_raw);
                let marker = embed_renderer::folder_list::emit_marker(
                    file_part,
                    source_path,
                    &params,
                );
                rewritten.push_str(&marker);
                rest = &rest[start + 3 + end + 2..];
                continue;
            }

            // Resolve via ContentGraph. Bail to no-rewrite if the
            // reference doesn't resolve — Stage 2's dispatcher will
            // emit the `[unresolved](moss-unresolved:…)` link form.
            let resolved =
                fuzzy_path::resolve_reference(file_part, graph, source_path);
            let target_path = match resolved {
                fuzzy_path::ResolvedRef::Found(p) => p,
                fuzzy_path::ResolvedRef::Unresolved => {
                    rewritten.push_str(&rest[start..start + 3 + end + 2]);
                    rest = &rest[start + 3 + end + 2..];
                    continue;
                }
            };
            let ext = target_path
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            // Markdown transclusion: `![[note.md]]` →
            // `<!-- moss-embed:note.md[#anchor] -->`.
            if ext == "md" || ext == "markdown" {
                let target_with_anchor = match anchor {
                    Some(a) => format!("{}#{}", target_path, a),
                    None => target_path,
                };
                rewritten.push_str("<!-- moss-embed:");
                rewritten.push_str(&target_with_anchor);
                rewritten.push_str(" -->");
                rest = &rest[start + 3 + end + 2..];
                continue;
            }
            // Deferred-handler embeds: `.ipynb` → notebook marker,
            // `.csv` / `.tsv` → table marker. These extensions route to
            // src-tauri marker handlers; the Stage 2 dispatcher would
            // also produce these markers, but it runs AFTER
            // `resolve_deferred_markers`, so pre-converting here keeps
            // the existing marker-handler pipeline working.
            let marker_prefix = match ext.as_str() {
                "ipynb" => Some("moss-embed-ipynb"),
                "csv" | "tsv" => Some("moss-embed-table"),
                _ => None,
            };
            if let Some(prefix) = marker_prefix {
                rewritten.push_str("<!-- ");
                rewritten.push_str(prefix);
                rewritten.push(':');
                rewritten.push_str(&target_path);
                rewritten.push_str(" -->");
                rest = &rest[start + 3 + end + 2..];
                continue;
            }
            // Other extensions (.pdf / .mp4 / .png / etc.) flow through
            // the Stage 2 dispatcher untouched — those renderers
            // produce HTML inline, not deferred markers.
            rewritten.push_str(&rest[start..start + 3 + end + 2]);
            rest = &rest[start + 3 + end + 2..];
        }
        rewritten.push_str(rest);
        output_lines.push(rewritten);
    }
    let mut out = output_lines.join("\n");
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub struct FrontmatterResolveResult {
    /// The frontmatter text with `[[wikilinks]]` replaced by resolved paths.
    pub content: String,
    /// Diagnostics for unresolved references.
    pub diagnostics: Vec<Diagnostic>,
}

/// Resolve `[[wikilink]]` patterns in frontmatter text to content graph paths.
///
/// Unlike body wikilink resolution (which produces markdown links like
/// `[text](url)`), this function replaces `[[ref]]` with just the resolved
/// path string.  Surrounding quotes are preserved.
///
/// # Examples
///
/// - `sidebar: "[[news]]"` → `sidebar: "news.md"` (or resolved path)
/// - `sidebar: [[news]]` → `sidebar: news.md`
/// - `cover: "[[photo.jpg]]"` → `cover: "assets/photo.jpg"`
/// - Unresolved: `[[missing]]` → `missing` (brackets stripped, diagnostic emitted)
///
/// The input `frontmatter` should include the delimiter(s) (e.g. `---`).
/// Wikilinks in delimiter lines are not expected but won't cause issues.
pub fn resolve_frontmatter_wikilinks(
    frontmatter: &str,
    graph: &ContentGraph,
    source_path: &str,
) -> FrontmatterResolveResult {
    let mut diagnostics = Vec::new();
    let mut result = String::with_capacity(frontmatter.len());
    let bytes = frontmatter.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for `![[` (embed wikilink) or `[[` (regular wikilink)
        // Embed prefix `!` is consumed — both resolve to the same path.
        // For embeds `![[path|attrs]]`, pipe content = display params (preserved).
        // For links `[[path|alias]]`, pipe content = alias text (discarded per Obsidian convention).
        let is_embed = i + 2 < len && bytes[i] == b'!' && bytes[i + 1] == b'[' && bytes[i + 2] == b'[';
        let is_wikilink = !is_embed && i + 1 < len && bytes[i] == b'[' && bytes[i + 1] == b'[';
        if is_embed || is_wikilink {
            let bracket_start = if is_embed { i + 3 } else { i + 2 };
            // Find closing `]]`
            if let Some(close_pos) = find_closing_brackets(bytes, bracket_start) {
                // Char-aligned: `bracket_start = i + 2` or `i + 3` where `i` is the
                // byte-cursor invariant of the outer loop (see else-branch comment),
                // and the offsets cross only ASCII bytes (`[`, `!`). `close_pos` is
                // returned by `find_closing_brackets` which scans for the ASCII pair
                // `]]`, so it lands on a char boundary.
                #[allow(clippy::string_slice)]
                let inner = &frontmatter[bracket_start..close_pos];

                // Split on | to separate path from pipe content
                let (ref_part, attrs_part) = crate::media::split_pipe(inner);

                // Resolve only the path part via the content graph
                let resolved_path = match graph.resolve_path(ref_part, source_path) {
                    Some(mut path) => {
                        // Only preserve pipe attrs for embed syntax (![[...|attrs]])
                        // For regular wikilinks ([[...|alias]]), discard the alias
                        if is_embed && !attrs_part.is_empty() {
                            path.push('|');
                            path.push_str(attrs_part);
                        }
                        path
                    }
                    None => {
                        diagnostics.push(Diagnostic {
                            message: format!(
                                "Unresolved frontmatter wikilink: [[{}]]",
                                ref_part
                            ),
                            source_path: source_path.to_string(),
                            reference: ref_part.to_string(),
                        });
                        // Strip brackets, use the path text as-is
                        let mut fallback = ref_part.to_string();
                        // Only preserve attrs for embed syntax
                        if is_embed && !attrs_part.is_empty() {
                            fallback.push('|');
                            fallback.push_str(attrs_part);
                        }
                        fallback
                    }
                };

                result.push_str(&resolved_path);
                i = close_pos + 2; // skip past `]]`
            } else {
                // No closing `]]` found — emit the opening chars as-is
                if is_embed {
                    result.push_str("![[");
                    i += 3;
                } else {
                    result.push('[');
                    i += 1;
                }
            }
        } else {
            // Byte-cursor invariant: `i` is always at a UTF-8 char boundary.
            //   * Initial value `i = 0` is a boundary.
            //   * In the wikilink branch above, `i` is reassigned to either
            //     `close_pos + 2` (close_pos is the byte index of the first `]`
            //     in the ASCII pair `]]`, so +2 also lands on an ASCII byte) or
            //     advanced by `+= 3` / `+= 1` past ASCII chars (`!`, `[`).
            //   * In this else branch, we read one full char from the boundary
            //     and advance by exactly its UTF-8 length, preserving the boundary.
            // Therefore slicing `frontmatter[i..]` here is safe, and the
            // `let-else { break }` is a defensive fallback: the loop guard
            // `i < len` already ensures at least one byte is available, but
            // bailing cleanly is cheaper than a panic if the invariant ever
            // breaks.
            #[allow(clippy::string_slice)]
            let Some(ch) = frontmatter[i..].chars().next() else {
                break;
            };
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    FrontmatterResolveResult {
        content: result,
        diagnostics,
    }
}

/// Find the position of the first `]]` in `bytes` starting from `start`.
/// Returns the byte index of the first `]` in the `]]` pair, or `None`.
fn find_closing_brackets(bytes: &[u8], start: usize) -> Option<usize> {
    let mut j = start;
    while j + 1 < bytes.len() {
        if bytes[j] == b']' && bytes[j + 1] == b']' {
            return Some(j);
        }
        // Wikilinks in frontmatter values are expected to be on a single line.
        // We allow multi-line scanning for robustness.
        j += 1;
    }
    None
}

/// Scan `content` starting from byte offset `scan_start` for the first
/// standalone `---` line.  Returns the byte position just past the
/// delimiter (including its trailing newline, if present).
fn find_delimiter(content: &str, scan_start: usize) -> Option<usize> {
    // Char-aligned: callers pass either 0 or `pos + 1` where `pos = content.find('\n')`
    // (an ASCII byte). Both values land on a UTF-8 char boundary.
    #[allow(clippy::string_slice)]
    let rest = &content[scan_start..];
    let mut offset = 0;
    for line in rest.lines() {
        if line.trim() == "---" {
            let close_abs = scan_start + offset + line.len();
            return if close_abs < content.len() && content.as_bytes()[close_abs] == b'\n' {
                Some(close_abs + 1)
            } else {
                Some(close_abs)
            };
        }
        offset += line.len() + 1; // +1 for '\n'
    }
    None
}

/// Split content into (frontmatter_including_delimiters, body).
///
/// Supports two frontmatter formats:
///
/// **Standard YAML** — content starts with `---\n`:
/// ```text
/// ---
/// title: Hello
/// ---
/// Body here.
/// ```
///
/// **Simplified** — content does NOT start with `---`, but contains a
/// standalone `---` line that separates frontmatter from body:
/// ```text
/// children: false
/// sidebar: "[[news]]"
/// ---
///
/// # Page Title
/// ```
///
/// In both cases the frontmatter portion includes the delimiter(s) and
/// any trailing newline after the closing `---`.  Returns
/// `(None, full_content)` when no frontmatter is detected.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    if content.starts_with("---") {
        // --- Standard YAML frontmatter ---

        // Find end of the opening `---` line.
        let after_opening = match content.find('\n') {
            Some(pos) => pos + 1,
            None => return (None, content),
        };

        // Search for a closing `---` line in the remainder.
        // Char-aligned: `split_pos` is computed by `find_delimiter` from
        // `scan_start + line.len() + (line.len() + 1)*N + (0 or 1)`. All
        // components are either char-aligned (`scan_start`, slices from `lines()`)
        // or single ASCII bytes (`'\n'`), so `split_pos` is on a char boundary.
        #[allow(clippy::string_slice)]
        match find_delimiter(content, after_opening) {
            Some(split_pos) => (Some(&content[..split_pos]), &content[split_pos..]),
            None => (None, content), // No closing delimiter — treat entire content as body.
        }
    } else {
        // --- Simplified frontmatter ---
        // Look for the first standalone `---` line.  Everything up to and
        // including that line (plus its trailing newline) is frontmatter;
        // everything after is body.
        // Same char-alignment rationale as above.
        #[allow(clippy::string_slice)]
        match find_delimiter(content, 0) {
            Some(split_pos) => (Some(&content[..split_pos]), &content[split_pos..]),
            None => (None, content), // No `---` found at all — no frontmatter.
        }
    }
}

/// Extract the parent directory from a `/`-separated path.
///
/// `"posts/hello.md"` -> `"posts"`, `"hello.md"` -> `""`.
pub(crate) fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        // Char-aligned: '/' is an ASCII byte, so `pos` is a char boundary.
        #[allow(clippy::string_slice)]
        Some(pos) => &path[..pos],
        None => "",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;
    use std::collections::HashMap;

    fn test_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("guide.md", "guide");
        b.add_file("note.md", "note");
        b.add_file("disclaimer.md", "disclaimer");
        b.add_file("assets/photo.jpg", "photo");
        b.add_headings(
            "guide.md",
            vec![("Setup".into(), "setup".into())],
        );
        b.add_blocks("guide.md", vec!["key-point".into()]);
        b.build()
    }

    fn test_files() -> HashMap<String, String> {
        let mut files = HashMap::new();
        files.insert(
            "disclaimer.md".into(),
            "---\ntitle: Disclaimer\n---\nThis is the disclaimer.\n\nSee [[guide]] for details."
                .into(),
        );
        files
    }

    fn mock_reader(files: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |path: &str| files.get(path).cloned()
    }

    // ----- split_frontmatter unit tests -----

    #[test]
    fn test_split_fm_present() {
        let input = "---\ntitle: Hello\n---\nBody here.";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("---\ntitle: Hello\n---\n"));
        assert_eq!(body, "Body here.");
    }

    #[test]
    fn test_split_fm_absent() {
        let input = "Just body content.";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn test_split_fm_no_closing() {
        let input = "---\ntitle: Hello\nno closing delimiter";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    // ----- split_frontmatter: simplified frontmatter tests -----

    #[test]
    fn test_split_simplified_frontmatter() {
        // Simplified format: no opening `---`, frontmatter lines before a `---` delimiter.
        let input = "sidebar: [[news]]\n---\n\n# Hello";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("sidebar: [[news]]\n---\n"));
        assert_eq!(body, "\n# Hello");
    }

    #[test]
    fn test_split_simplified_preserves_body() {
        let input = "children: false\nuid: a48746ca\n---\n\n# Page Title\n\nBody content here\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("children: false\nuid: a48746ca\n---\n"));
        assert_eq!(body, "\n# Page Title\n\nBody content here\n");
    }

    #[test]
    fn test_split_no_delimiter() {
        // No `---` at all — everything is body, no frontmatter.
        let input = "Just some content\nwith multiple lines\nbut no delimiter";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn test_split_simplified_with_quoted_wikilink() {
        let input = "sidebar: \"[[news]]\"\n---\nBody text";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("sidebar: \"[[news]]\"\n---\n"));
        assert_eq!(body, "Body text");
    }

    #[test]
    fn test_split_simplified_empty_body() {
        // Simplified frontmatter with nothing after the delimiter.
        let input = "title: Test\n---\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("title: Test\n---\n"));
        assert_eq!(body, "");
    }

    #[test]
    fn test_split_simplified_delimiter_at_eof_no_newline() {
        // Simplified frontmatter where `---` is the last line with no trailing newline.
        let input = "title: Test\n---";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("title: Test\n---"));
        assert_eq!(body, "");
    }

    #[test]
    fn test_split_simplified_multiple_dashes_in_body() {
        // Only the FIRST `---` should be treated as the delimiter.
        let input = "title: Test\n---\n\nSome body\n---\nMore body";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("title: Test\n---\n"));
        assert_eq!(body, "\nSome body\n---\nMore body");
    }

    // ----- Integration tests for resolve_content -----

    #[test]
    fn test_full_resolve_pipeline() {
        let graph = test_graph();
        let files = test_files();

        let input = "---\ntitle: Test\n---\nSee [[guide#Setup]] for help.\n\nImportant point. ^my-block\n\n> [!warning] Watch Out\n> Be careful here.";

        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // Frontmatter preserved
        assert!(result.content_markdown.starts_with("---\ntitle: Test\n---\n"));

        // Phase 3 PR2: `resolve_content` no longer resolves body wikilinks
        // — that's the Stage 2 dispatcher's job in
        // `src-tauri/src/build/markdown/pipeline.rs::transform_events`.
        // The `[[guide#Setup]]` wikilink passes through unchanged here.
        assert!(result.content_markdown.contains("[[guide#Setup]]"));

        // Block ref transformed
        assert!(result.content_markdown.contains("<span id=\"my-block\"></span>"));
        assert_eq!(result.block_ids, vec!["my-block"]);

        // Phase 4 PR4 (2026-05-27): typed Block::Callout was added to
        // the AST parser (`ast/parser.rs`'s Tag::BlockQuote arm) WITH
        // proper Obsidian-alias canonicalization. The Stage 1
        // `resolve/callouts.rs` pass STAYS until PR7a flips production
        // rendering to `render_document` (deleting Stage 1 before then
        // would regress production output to a plain `<blockquote>`).
        // So `resolve_content` still emits the canonical callout HTML
        // here, matching the pre-PR4 behavior.
        assert!(
            result.content_markdown.contains(r#"class="callout""#),
            "Stage 1 callout transformation still active until PR7a flip"
        );
    }

    #[test]
    fn test_frontmatter_preserved() {
        let graph = test_graph();
        let files = HashMap::new();

        let input = "---\ntitle: My Page\ntags:\n  - rust\n  - wasm\n---\nPlain body.";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert!(result.content_markdown.starts_with("---\ntitle: My Page\ntags:\n  - rust\n  - wasm\n---\n"));
        assert!(result.content_markdown.ends_with("Plain body."));
    }

    #[test]
    fn test_no_obsidian_syntax() {
        let graph = test_graph();
        let files = HashMap::new();

        let input = "---\ntitle: Plain\n---\nJust a plain paragraph.\n\nAnother paragraph.";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert_eq!(result.content_markdown, input);
        assert!(result.outgoing_links.is_empty());
        assert!(result.diagnostics.is_empty());
        assert!(result.block_ids.is_empty());
        assert!(result.embed_deps.is_empty());
    }

    #[test]
    fn test_embedded_wikilinks_resolved() {
        let graph = test_graph();
        let files = test_files();

        // disclaimer.md body contains `See [[guide]] for details.`
        // Phase 3 PR2: the embedded body's wikilink is no longer
        // resolved by `resolve_content`; the Stage 2 dispatcher in
        // src-tauri handles it. `resolve_content` lowers
        // `![[disclaimer]]` into the `<!-- moss-embed:disclaimer.md -->`
        // marker, then `resolve_embeds` inlines the disclaimer body
        // verbatim — wikilinks inside survive into the markdown
        // returned here.
        let input = "![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // The embedded body's wikilink survives as raw `[[guide]]`
        // (handed off to Stage 2 downstream).
        assert!(
            result.content_markdown.contains("[[guide]]"),
            "Expected raw wikilink from embedded content, got: {}",
            result.content_markdown
        );
        // The disclaimer body text should be present
        assert!(result.content_markdown.contains("This is the disclaimer."));
    }

    #[test]
    fn test_diagnostics_merged() {
        let graph = test_graph();
        let files = HashMap::new();

        // Phase 3 PR2: wikilink unresolved diagnostics now surface from
        // the Stage 2 dispatcher in src-tauri. `resolve_content` only
        // surfaces diagnostics from passes it still runs (transclusion
        // / deferred markers / markdown_refs / markdown_links / block
        // refs / callouts). `![[missing]]` with no extension resolves
        // to Unresolved in the lowering pass — but the lowering pass
        // leaves the raw `![[missing]]` for Stage 2 to handle and does
        // NOT emit a diagnostic itself. So this test asserts the new
        // contract: zero diagnostics for body wikilinks at this layer.
        let input = "[[nonexistent]] and ![[missing]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // Body wikilinks pass through; no diagnostics from this layer.
        assert!(
            result.diagnostics.is_empty(),
            "Expected zero diagnostics post-PR2 (body wikilinks deferred), got: {:?}",
            result.diagnostics
        );
        // Raw wikilinks pass through to the markdown handed back.
        assert!(result.content_markdown.contains("[[nonexistent]]"));
        assert!(result.content_markdown.contains("![[missing]]"));
    }

    #[test]
    fn test_outgoing_links_tracked() {
        let graph = test_graph();
        let files = test_files();

        // Phase 3 PR2: body wikilink outgoing-links are populated by
        // the Stage 2 dispatcher in src-tauri (not by `resolve_content`).
        // What this layer still populates: markdown_refs / markdown_links
        // / block_refs / callouts results. The wikilink body links
        // `[[guide]]` and `![[disclaimer]]` pass through to Stage 2.
        let input = "[[guide]]\n\n![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // The disclaimer body got inlined (via `<!-- moss-embed -->`
        // lowering + resolve_embeds), but its `[[guide]]` is now raw
        // markdown for Stage 2 — none of these appear in
        // outgoing_links from this layer.
        let wikilinks: Vec<_> = result
            .outgoing_links
            .iter()
            .filter(|l| l.link_type == LinkType::Wikilink)
            .collect();
        let embeds: Vec<_> = result
            .outgoing_links
            .iter()
            .filter(|l| l.link_type == LinkType::Embed)
            .collect();

        assert!(
            wikilinks.is_empty(),
            "Expected zero wikilink outgoing links from resolve_content post-PR2; got {}: {:?}",
            wikilinks.len(),
            wikilinks
        );
        // No-op smoke check that the rest of the assertions still
        // exercise the embed-tracking path through `embed_deps`.
        let _ = embeds; // not populated by this layer either
        assert!(
            !result.embed_deps.is_empty(),
            "Expected at least 1 embed outgoing link"
        );
    }

    #[test]
    fn test_embed_deps_tracked() {
        let graph = test_graph();
        let files = test_files();

        let input = "![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert!(
            result
                .embed_deps
                .contains(&("disclaimer.md".to_string(), "note.md".to_string())),
            "Expected embed dep (disclaimer.md, note.md), got: {:?}",
            result.embed_deps
        );
    }

    // ----- Regression test for deeply-nested Unicode paths (#342) -----

    #[test]
    fn test_deeply_nested_unicode_bare_filename() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("assets/d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg", "d9512f2d");
        b.add_file("articles/\u{65e0}\u{7528}\u{4e4b}\u{65c5}/\u{771f}\u{6b63}\u{7684}\u{65c5}\u{7a0b}.md", "articles/\u{65e0}\u{7528}\u{4e4b}\u{65c5}/\u{771f}\u{6b63}\u{7684}\u{65c5}\u{7a0b}");
        let graph = b.build();
        let files = HashMap::new();

        let input = "---\ndate: 2025-12-03\n---\n![](d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg)\n\nSome text.";
        let result = resolve_content(
            "articles/\u{65e0}\u{7528}\u{4e4b}\u{65c5}/\u{771f}\u{6b63}\u{7684}\u{65c5}\u{7a0b}.md",
            input, &graph, &mock_reader(&files)
        );

        assert!(
            result.content_markdown.contains("../../assets/d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg"),
            "Expected resolved path with ../../assets/, got: {}",
            result.content_markdown
        );
    }

    // ----- Integration test for markdown image bare-filename resolution -----

    #[test]
    fn test_bare_filename_image_resolved_in_pipeline() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("guide.md", "guide");
        b.add_file("note.md", "note");
        b.add_file("assets/photo.jpg", "photo");
        b.add_headings(
            "guide.md",
            vec![("Setup".into(), "setup".into())],
        );
        b.add_blocks("guide.md", vec!["key-point".into()]);
        let graph = b.build();
        let files = HashMap::new();

        let input = "---\ntitle: Test\n---\n![My Image](photo.jpg)\n\nSome text.";
        let result = resolve_content("articles/post.md", input, &graph, &mock_reader(&files));

        // Frontmatter preserved
        assert!(result.content_markdown.starts_with("---\ntitle: Test\n---\n"));

        // Bare filename resolved to correct relative path
        assert!(
            result.content_markdown.contains("![My Image](../assets/photo.jpg)"),
            "Expected resolved image path, got: {}",
            result.content_markdown
        );

        // Outgoing link tracked
        let standard_links: Vec<_> = result
            .outgoing_links
            .iter()
            .filter(|l| l.link_type == LinkType::Standard)
            .collect();
        assert_eq!(
            standard_links.len(),
            1,
            "Expected 1 standard outgoing link, got {}: {:?}",
            standard_links.len(),
            standard_links
        );
        assert_eq!(standard_links[0].target_path, "assets/photo.jpg");
    }

    // ----- resolve_frontmatter_wikilinks unit tests -----

    fn fm_test_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("news.md", "news");
        b.add_file("news/index.md", "news-index");
        b.add_file("assets/photo.jpg", "photo");
        b.add_file("posts/ch-1.md", "ch-1");
        b.add_file("posts/ch-2.md", "ch-2");
        b.build()
    }

    #[test]
    fn test_fm_wikilink_basic_quoted() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: \"[[news]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\nsidebar: \"news.md\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_unquoted() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: [[news]]\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\nsidebar: news.md\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_cover_image() {
        let graph = fm_test_graph();
        let fm = "---\ncover: \"[[photo.jpg]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\ncover: \"assets/photo.jpg\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_folder_note() {
        // [[news]] when news/index.md exists should resolve to folder note path.
        // But news.md also exists and is an exact stem match, so it resolves to news.md.
        // Let's build a graph where only the folder note exists.
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("news/index.md", "news-index");
        let graph = b.build();

        let fm = "---\nsidebar: \"[[news]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\nsidebar: \"news/index.md\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_array_items() {
        let graph = fm_test_graph();
        let fm = "---\nseries: [\"[[ch-1]]\", \"[[ch-2]]\"]\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(
            result.content,
            "---\nseries: [\"posts/ch-1.md\", \"posts/ch-2.md\"]\n---\n"
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_unresolved() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: \"[[missing]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        // Brackets stripped, inner text used as fallback
        assert_eq!(result.content, "---\nsidebar: \"missing\"\n---\n");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].reference, "missing");
        assert_eq!(result.diagnostics[0].source_path, "index.md");
        assert!(result.diagnostics[0].message.contains("[[missing]]"));
    }

    #[test]
    fn test_fm_wikilink_multiple() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: \"[[news]]\"\ncover: \"[[photo.jpg]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(
            result.content,
            "---\nsidebar: \"news.md\"\ncover: \"assets/photo.jpg\"\n---\n"
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_no_wikilinks() {
        let graph = fm_test_graph();
        let fm = "---\ntitle: Hello\ntags:\n  - rust\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, fm);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_simplified_frontmatter_wikilink() {
        let graph = fm_test_graph();
        // Simplified frontmatter (no opening ---)
        let fm = "sidebar: \"[[news]]\"\nchildren: false\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "sidebar: \"news.md\"\nchildren: false\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_unclosed_wikilink_preserved() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: \"[[unclosed\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        // No closing ]] — the [[ is preserved as-is
        assert_eq!(result.content, "---\nsidebar: \"[[unclosed\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_mixed_resolved_and_unresolved() {
        let graph = fm_test_graph();
        let fm = "---\nsidebar: \"[[news]]\"\nrelated: \"[[missing]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(
            result.content,
            "---\nsidebar: \"news.md\"\nrelated: \"missing\"\n---\n"
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].reference, "missing");
    }

    // ----- Pipe-aware frontmatter wikilink resolution -----

    #[test]
    fn test_fm_wikilink_alias_discarded() {
        // [[photo.jpg|left]] — pipe content is alias (Obsidian convention), discarded
        let graph = fm_test_graph();
        let fm = "---\ncover: \"[[photo.jpg|left]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\ncover: \"assets/photo.jpg\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_embed_wikilink_with_attrs() {
        // ![[photo.jpg|cover left]] — embed syntax preserves display params
        let graph = fm_test_graph();
        let fm = "---\ncover: \"![[photo.jpg|cover left]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(
            result.content,
            "---\ncover: \"assets/photo.jpg|cover left\"\n---\n"
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_no_attrs_unchanged() {
        // [[photo.jpg]] without pipe should work exactly as before
        let graph = fm_test_graph();
        let fm = "---\ncover: \"[[photo.jpg]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\ncover: \"assets/photo.jpg\"\n---\n");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_fm_wikilink_alias_unresolved_discarded() {
        // [[missing.jpg|left]] — unresolved, alias still discarded
        let graph = fm_test_graph();
        let fm = "---\ncover: \"[[missing.jpg|left]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(result.content, "---\ncover: \"missing.jpg\"\n---\n");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].reference, "missing.jpg");
    }

    #[test]
    fn test_fm_embed_wikilink_with_fit_and_position() {
        // ![[photo.jpg|contain top-right]] — embed syntax preserves both keywords
        let graph = fm_test_graph();
        let fm = "---\ncover: \"![[photo.jpg|contain top-right]]\"\n---\n";
        let result = resolve_frontmatter_wikilinks(fm, &graph, "index.md");
        assert_eq!(
            result.content,
            "---\ncover: \"assets/photo.jpg|contain top-right\"\n---\n"
        );
        assert!(result.diagnostics.is_empty());
    }

    // ----- Frontmatter wikilinks are now resolved to paths -----

    #[test]
    fn test_simplified_frontmatter_wikilink_resolved_to_path() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("news.md", "news");
        let graph = b.build();
        let files = HashMap::new();

        // Simplified frontmatter (no leading ---) with a wikilink in sidebar value.
        // Frontmatter wikilinks ARE still resolved here (to a path).
        let input = "children: false\nsidebar: \"[[news]]\"\nuid: a48746ca\n---\n\n# Welcome\n\nBody with [[news]] link.";
        let result = resolve_content("index.md", input, &graph, &mock_reader(&files));

        // Frontmatter wikilink [[news]] resolved to path "news.md", quotes preserved.
        assert!(
            result.content_markdown.starts_with("children: false\nsidebar: \"news.md\"\nuid: a48746ca\n---\n"),
            "Frontmatter wikilink not resolved to path: {}",
            result.content_markdown
        );

        // Phase 3 PR2: body wikilink `[[news]]` passes through as raw
        // markdown — Stage 2 in src-tauri resolves it via the
        // `dispatch_wikilink_embed` arm in `transform_events`.
        assert!(
            result.content_markdown.contains("[[news]]"),
            "Expected body wikilink to pass through verbatim, got: {}",
            result.content_markdown
        );
    }

    #[test]
    fn test_frontmatter_embed_wikilink_stripped() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("photos/hero.jpg", "hero");
        let graph = b.build();
        let files = HashMap::new();

        // Embed wikilink ![[hero.jpg]] in frontmatter cover — the ! prefix should be consumed.
        let input = "cover: \"![[hero.jpg]]\"\n---\n\n# Page";
        let result = resolve_content("index.md", input, &graph, &mock_reader(&files));

        // Should resolve to path without ! prefix
        assert!(
            result.content_markdown.starts_with("cover: \"photos/hero.jpg\"\n---"),
            "Embed wikilink ! prefix not stripped: {}",
            result.content_markdown
        );
    }

    #[test]
    fn test_frontmatter_embed_wikilink_with_attrs() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("photos/hero.jpg", "hero");
        let graph = b.build();
        let files = HashMap::new();

        // Embed wikilink with display attrs: ![[hero.jpg|cover left]]
        let input = "cover: \"![[hero.jpg|cover left]]\"\n---\n\n# Page";
        let result = resolve_content("index.md", input, &graph, &mock_reader(&files));

        // Should resolve path and preserve attrs
        assert!(
            result.content_markdown.starts_with("cover: \"photos/hero.jpg|cover left\"\n---"),
            "Embed wikilink with attrs not resolved correctly: {}",
            result.content_markdown
        );
    }

    #[test]
    fn standard_markdown_link_resolves_folder_note() {
        // Graph contains only 文字/文字.md, not 文字.md.
        // A link [文字](文字.md) from root index.md should resolve via
        // ContentGraph::resolve_path's folder-note fallback.
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("文字/文字.md", "writings");
        let graph = b.build();

        let files = HashMap::new();
        let result = resolve_content(
            "index.md",
            "[文字](文字.md)\n",
            &graph,
            &mock_reader(&files),
        );

        // The resolver should have rewritten 文字.md to 文字/文字.md.
        assert!(
            result.content_markdown.contains("文字/文字.md"),
            "expected folder-note resolution; got: {}",
            result.content_markdown
        );
    }

    #[test]
    fn test_frontmatter_link_wikilink_alias_discarded() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "index");
        b.add_file("photos/hero.jpg", "hero");
        let graph = b.build();
        let files = HashMap::new();

        // Regular wikilink [[hero.jpg|My Hero]] — pipe content is alias, should be discarded
        let input = "cover: \"[[hero.jpg|My Hero]]\"\n---\n\n# Page";
        let result = resolve_content("index.md", input, &graph, &mock_reader(&files));

        // Should resolve path but discard alias (Obsidian convention: pipe = alias in [[...]])
        assert!(
            result.content_markdown.starts_with("cover: \"photos/hero.jpg\"\n---"),
            "Link wikilink alias should be discarded, got: {}",
            result.content_markdown
        );
    }
}
