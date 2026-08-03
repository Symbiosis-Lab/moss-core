//! Pure markdown reference extractor — zero I/O, no resolve, no indexes.
//!
//! Scans raw markdown source for every reference token (wikilink / embed /
//! markdown link / markdown image) and returns the raw text plus byte offsets
//! covering the whole token. The offsets let callers rewrite the source without
//! re-scanning.
//!
//! **No resolution** happens here. The caller (src-tauri) resolves each
//! `RawRef` against the project's indexes.
//!
//! Recognition runs over [`crate::inert_regions`]'s mask rather than a
//! private fence tracker, so this scanner and
//! [`crate::ast::shortcode_extract::shortcode_asset_spans`] give one
//! identical answer to "which bytes are live syntax". Two behaviours changed
//! when the private tracker was deleted (2026-08-03): references inside an
//! authored `<!-- … -->` comment and inside an indented code block are no
//! longer extracted. The old doc justified scanning comments with
//! build-internal `<!-- moss-embed:… -->` sentinels, which never appear in
//! the author files this module's only consumer (src-tauri's
//! `editor::ref_scan`) reads from disk.

/// Which surface syntax produced this reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefSyntax {
    /// `[[stem]]` — bare wikilink, stem only (no `/`)
    WikilinkStem,
    /// `[[a/b]]` — wikilink with a path component
    WikilinkPath,
    /// `![[x]]` — embed, bare stem
    WikilinkStemEmbed,
    /// `![[a/b]]` — embed, path
    WikilinkPathEmbed,
    /// `[[stem|Display]]` — wikilink with alias
    WikilinkAliased { display: String },
    /// `![[stem|Display]]` / `![[stem|500]]` — embed with pothole
    WikilinkAliasedEmbed { display: String },
    /// `[label](path)` — standard markdown link
    MarkdownLink { label: String },
    /// `![alt](path)` — standard markdown image
    MarkdownImage { alt: String },
}

/// A raw reference extracted from a markdown source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRef {
    /// The resolved/target text (the inner `stem`, `a/b`, or `path` part — no
    /// brackets, no alias, no pothole). This is the string to pass to the
    /// classifier.
    pub text: String,
    /// Which syntax form produced this reference.
    pub syntax: RefSyntax,
    /// Byte offset in the source string where the token starts (inclusive).
    pub byte_from: usize,
    /// Byte offset in the source string where the token ends (exclusive).
    pub byte_to: usize,
}

/// Extract all markdown references from `source`.
///
/// Recognition runs over [`crate::inert_regions::mask_inert`], the one
/// shared answer to "which bytes are not live syntax" — so references
/// inside fenced code blocks, indented code blocks, inline code spans and
/// HTML comments are skipped. Every *string* (`text`, `label`, `alt`, the
/// wikilink alias) is sliced from the ORIGINAL source, because the mask
/// blanks inline code spans and would otherwise corrupt a label like
/// ``[a `b` c](x.md)``.
///
/// External URLs (`http://…`, `https://…`, `//`, `mailto:`, `tel:`, `data:`)
/// are included as `MarkdownLink` / `MarkdownImage` — the caller decides
/// whether to filter them out.
pub fn extract_md_references(source: &str) -> Vec<RawRef> {
    let mask = crate::inert_regions::mask_inert(source);
    let bytes = mask.as_bytes();
    let len = bytes.len();
    let mut refs = Vec::new();
    let mut i = 0;

    while i < len {
        // ── Backslash escape: `\[[note]]` / `\[t](p)` are NOT references ──
        // An escape is not an inert region (the mask leaves it alone), so it
        // stays a case here. Skip the backslash and the next char so the
        // escaped bracket can't start a reference token. Advance by a full
        // char (not a byte) so `i` stays on a UTF-8 boundary for later slices.
        if bytes[i] == b'\\' {
            i += 1; // past the backslash (ASCII, boundary-safe)
            if i < len {
                // SAFETY: `i` is a char boundary here; read one full char.
                #[allow(clippy::string_slice)]
                if let Some(ch) = source[i..].chars().next() {
                    i += ch.len_utf8();
                }
            }
            continue;
        }

        // ── Wikilink / embed: ![[…]] or [[…]] ───────────────────────────
        let is_embed_wikilink = i + 4 < len
            && bytes[i] == b'!'
            && bytes[i+1] == b'['
            && bytes[i+2] == b'[';
        let is_wikilink = !is_embed_wikilink
            && i + 3 < len
            && bytes[i] == b'['
            && bytes[i+1] == b'[';

        if is_embed_wikilink || is_wikilink {
            let token_start = i;
            let inner_start = if is_embed_wikilink { i + 3 } else { i + 2 };
            // Find closing ]] — in the MASK, so a `]]` hidden in inline code
            // does not close a live wikilink.
            if let Some(close) = find_double_bracket(bytes, inner_start) {
                // SAFETY: inner_start and close are valid UTF-8 char boundaries
                // because we only advance past ASCII bytes ([, !, ]) to reach them.
                #[allow(clippy::string_slice)]
                let inner = &source[inner_start..close];
                let token_end = close + 2;
                // Split on | for alias/pothole
                let (path_part, pipe_part) = match inner.split_once('|') {
                    Some((before, after)) => (before, Some(after)),
                    None => (inner, None),
                };
                // Only record non-empty targets
                if !path_part.trim().is_empty() {
                    let text = path_part.trim().to_string();
                    let has_slash = text.contains('/');
                    let syntax = match (is_embed_wikilink, pipe_part) {
                        (false, None) => {
                            if has_slash { RefSyntax::WikilinkPath } else { RefSyntax::WikilinkStem }
                        }
                        (false, Some(alias)) => RefSyntax::WikilinkAliased { display: alias.to_string() },
                        (true, None) => {
                            if has_slash { RefSyntax::WikilinkPathEmbed } else { RefSyntax::WikilinkStemEmbed }
                        }
                        (true, Some(pot)) => RefSyntax::WikilinkAliasedEmbed { display: pot.to_string() },
                    };
                    refs.push(RawRef { text, syntax, byte_from: token_start, byte_to: token_end });
                }
                i = token_end;
                continue;
            }
        }

        // ── Markdown image ![alt](path) ──────────────────────────────────
        if i + 3 < len && bytes[i] == b'!' && bytes[i+1] == b'[' {
            if let Some((alt, path, end)) = parse_md_link(source, bytes, i + 1) {
                let token_start = i;
                refs.push(RawRef {
                    text: path,
                    syntax: RefSyntax::MarkdownImage { alt },
                    byte_from: token_start,
                    byte_to: end,
                });
                i = end;
                continue;
            }
        }

        // ── Markdown link [label](path) ──────────────────────────────────
        if bytes[i] == b'[' {
            // Guard: not a wikilink (already handled above)
            if i + 1 < len && bytes[i+1] != b'[' {
                if let Some((label, path, end)) = parse_md_link(source, bytes, i) {
                    refs.push(RawRef {
                        text: path,
                        syntax: RefSyntax::MarkdownLink { label },
                        byte_from: i,
                        byte_to: end,
                    });
                    i = end;
                    continue;
                }
            }
        }

        i += 1;
    }

    refs
}

/// Find the byte index of the first `]]` in `bytes` at or after `start`.
/// Returns the index of the first `]` in the `]]` pair, or `None`.
fn find_double_bracket(bytes: &[u8], start: usize) -> Option<usize> {
    let mut j = start;
    while j + 1 < bytes.len() {
        if bytes[j] == b']' && bytes[j+1] == b']' {
            return Some(j);
        }
        // Bail on newline — wikilinks are single-line
        if bytes[j] == b'\n' {
            return None;
        }
        j += 1;
    }
    None
}

/// Parse a `[label](path)` or `![alt](path)` link starting at `bracket_pos`
/// (the position of the opening `[`).
/// Returns `(label_or_alt, path, byte_end)` or `None`.
fn parse_md_link(source: &str, bytes: &[u8], bracket_pos: usize) -> Option<(String, String, usize)> {
    let len = bytes.len();
    // Find closing ] — but respect nested brackets and bail on newline
    let mut depth = 0usize;
    let mut j = bracket_pos;
    while j < len {
        match bytes[j] {
            b'[' => { depth += 1; j += 1; }
            b']' => {
                depth -= 1;
                if depth == 0 { break; }
                j += 1;
            }
            b'\n' => return None,
            _ => { j += 1; }
        }
    }
    if j >= len || bytes[j] != b']' { return None; }
    let label_start = bracket_pos + 1;
    let label_end = j;
    #[allow(clippy::string_slice)]
    let label = source[label_start..label_end].to_string();

    // Expect `(` immediately after `]`
    let paren_open = j + 1;
    if paren_open >= len || bytes[paren_open] != b'(' { return None; }

    // Find closing `)` — respect nesting, bail on newline
    let mut depth = 0usize;
    let mut k = paren_open;
    while k < len {
        match bytes[k] {
            b'(' => { depth += 1; k += 1; }
            b')' => {
                depth -= 1;
                if depth == 0 { break; }
                k += 1;
            }
            b'\n' => return None,
            _ => { k += 1; }
        }
    }
    if k >= len || bytes[k] != b')' { return None; }
    let path_start = paren_open + 1;
    let path_end = k;
    #[allow(clippy::string_slice)]
    let path_raw = source[path_start..path_end].trim().to_string();
    // Strip optional title: `path "title"` → path
    let path = strip_link_title(&path_raw);
    let token_end = k + 1;

    Some((label, path, token_end))
}

/// Strip an optional CommonMark link title from a raw link destination string.
/// `path "My Title"` → `path`, `path 'title'` → `path`, `path (title)` → `path`.
/// If no title is present, returns the input unchanged.
fn strip_link_title(raw: &str) -> String {
    let raw = raw.trim();
    // Find the last whitespace-separated token that looks like a title
    if let Some(ws) = raw.rfind(|c: char| c.is_ascii_whitespace()) {
        let (path_part, maybe_title) = raw.split_at(ws);
        let maybe_title = maybe_title.trim();
        let is_title = (maybe_title.starts_with('"') && maybe_title.ends_with('"'))
            || (maybe_title.starts_with('\'') && maybe_title.ends_with('\''))
            || (maybe_title.starts_with('(') && maybe_title.ends_with(')'));
        if is_title {
            return path_part.trim().to_string();
        }
    }
    raw.to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "md_extract_tests.rs"]
mod tests;
