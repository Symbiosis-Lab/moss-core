//! Pure markdown reference extractor — zero I/O, no resolve, no indexes.
//!
//! Scans raw markdown source for every reference token (wikilink / embed /
//! markdown link / markdown image) and returns the raw text plus byte offsets
//! covering the whole token. The offsets let callers rewrite the source without
//! re-scanning.
//!
//! **No resolution** happens here. The caller (src-tauri) resolves each
//! `RawRef` against the project's indexes.

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
/// Skips content inside fenced code blocks (` ``` ` / `~~~`) and inline
/// code spans.  Does **not** skip HTML comments — `<!-- moss-embed:… -->`
/// is build-internal and not a user-authored reference.
///
/// External URLs (`http://…`, `https://…`, `//`, `mailto:`, `tel:`, `data:`)
/// are included as `MarkdownLink` / `MarkdownImage` — the caller decides
/// whether to filter them out.
pub fn extract_md_references(source: &str) -> Vec<RawRef> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut refs = Vec::new();
    let mut i = 0;

    // Fenced block tracking: Some(char) while inside a fence.
    let mut fence_char: Option<u8> = None;
    // Track line starts for fence detection.
    let mut line_start = 0;

    while i < len {
        // ── Newline: advance line_start, check for fence ─────────────────
        if bytes[i] == b'\n' {
            i += 1;
            line_start = i;
            continue;
        }

        // ── At start of a line: check for fence open/close ───────────────
        if i == line_start {
            // Skip leading whitespace (up to 3 spaces per CommonMark for fences)
            let mut j = i;
            while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') && j - i < 4 {
                j += 1;
            }
            // Check for ``` or ~~~
            let fence_cand = if j + 2 < len && bytes[j] == b'`' && bytes[j+1] == b'`' && bytes[j+2] == b'`' {
                Some(b'`')
            } else if j + 2 < len && bytes[j] == b'~' && bytes[j+1] == b'~' && bytes[j+2] == b'~' {
                Some(b'~')
            } else {
                None
            };
            if let Some(fc) = fence_cand {
                if let Some(cur_fc) = fence_char {
                    if cur_fc == fc {
                        // Closing fence: rest of line must not contain fc
                        let mut k = j + 3;
                        while k < len && bytes[k] == fc { k += 1; }
                        // skip spaces
                        while k < len && bytes[k] == b' ' { k += 1; }
                        if k >= len || bytes[k] == b'\n' {
                            fence_char = None;
                            // Advance past the closing-fence line itself. Without
                            // this, `i` still points at the fence line and the
                            // backtick handler below would consume the rest of
                            // the file, silently dropping every reference AFTER a
                            // fenced code block.
                            while i < len && bytes[i] != b'\n' {
                                i += 1;
                            }
                            continue;
                        }
                    }
                } else {
                    fence_char = Some(fc);
                }
            }
        }

        // Inside a fenced block: skip until newline
        if fence_char.is_some() {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // ── Inline code span: skip ──────────────────────────────────────
        if bytes[i] == b'`' {
            // Count backtick run
            let mut n = 0;
            while i + n < len && bytes[i + n] == b'`' { n += 1; }
            let start = i;
            i += n;
            // Find matching run of n backticks
            while i < len {
                if bytes[i] == b'`' {
                    let mut m = 0;
                    while i + m < len && bytes[i + m] == b'`' { m += 1; }
                    if m == n {
                        i += m;
                        break;
                    }
                    i += m;
                } else {
                    i += 1;
                }
            }
            let _ = start;
            continue;
        }

        // ── Backslash escape: `\[[note]]` / `\[t](p)` are NOT references ──
        // Skip the backslash and the next char so the escaped bracket can't
        // start a reference token. Advance by a full char (not a byte) so `i`
        // stays on a UTF-8 boundary for later string slices.
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
            // Find closing ]]
            if let Some(close) = find_double_bracket(bytes, inner_start) {
                // SAFETY: inner_start and close are valid UTF-8 char boundaries
                // because we only advance past ASCII bytes ([, !, ]) to reach them.
                #[allow(clippy::string_slice)]
                let inner = &source[inner_start..close];
                let token_end = close + 2;
                // Split on | for alias/pothole
                let (path_part, pipe_part) = match inner.find('|') {
                    Some(p) => (&inner[..p], Some(&inner[p+1..])),
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
mod tests {
    use super::*;

    #[test]
    fn wikilink_stem() {
        let src = "See [[note]] for details.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "note");
        assert_eq!(refs[0].syntax, RefSyntax::WikilinkStem);
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[note]]");
    }

    #[test]
    fn wikilink_path() {
        let src = "See [[a/b]] here.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "a/b");
        assert_eq!(refs[0].syntax, RefSyntax::WikilinkPath);
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[a/b]]");
    }

    #[test]
    fn wikilink_stem_embed() {
        let src = "![[x]] is an embed.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "x");
        assert_eq!(refs[0].syntax, RefSyntax::WikilinkStemEmbed);
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[x]]");
    }

    #[test]
    fn wikilink_path_embed() {
        let src = "![[a/b]] embedded.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "a/b");
        assert_eq!(refs[0].syntax, RefSyntax::WikilinkPathEmbed);
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[a/b]]");
    }

    #[test]
    fn wikilink_aliased() {
        let src = "See [[stem|Display]] here.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "stem");
        assert!(matches!(&refs[0].syntax, RefSyntax::WikilinkAliased { display } if display == "Display"));
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[stem|Display]]");
    }

    #[test]
    fn wikilink_aliased_embed() {
        let src = "![[stem|500]] wide embed.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "stem");
        assert!(matches!(&refs[0].syntax, RefSyntax::WikilinkAliasedEmbed { display } if display == "500"));
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[stem|500]]");
    }

    #[test]
    fn markdown_link() {
        let src = "Click [here](page.md) now.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "page.md");
        assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownLink { label } if label == "here"));
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[here](page.md)");
    }

    #[test]
    fn markdown_image() {
        let src = "![alt text](img.png) here.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "img.png");
        assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownImage { alt } if alt == "alt text"));
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![alt text](img.png)");
    }

    #[test]
    fn external_link_included() {
        let src = "[foo](https://example.com)";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "https://example.com");
        assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownLink { .. }));
    }

    #[test]
    fn skip_fenced_code_block() {
        let src = "```\n[[note]]\n```\nAfter.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 0, "wikilink inside fenced block should be skipped");
    }

    #[test]
    fn ref_after_fence_is_found() {
        // Regression: the closing-fence line must be advanced past, otherwise
        // the backtick handler swallows the rest of the file and drops every
        // reference after a fenced block.
        let src = "```\n[[skip]]\n```\n[[find]]";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1, "exactly the ref after the fence is found, got: {:?}", refs);
        assert_eq!(refs[0].text, "find");
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[find]]");
    }

    #[test]
    fn backslash_escaped_refs_are_skipped() {
        // `\[[note]]` and `\[t](p)` are escaped and must NOT be extracted.
        let src = "Escaped \\[[note]] and \\[t](p.md) but [[real]] counts.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1, "only the unescaped ref should be found, got: {:?}", refs);
        assert_eq!(refs[0].text, "real");
    }

    #[test]
    fn skip_inline_code_span() {
        let src = "In `` [[note]] `` code.";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 0, "wikilink inside inline code should be skipped");
    }

    #[test]
    fn multiple_refs_byte_offsets() {
        let src = "[[a]] and [[b]]";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 2);
        assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[a]]");
        assert_eq!(&src[refs[1].byte_from..refs[1].byte_to], "[[b]]");
    }

    #[test]
    fn aliased_embed_preserves_alias() {
        // ![[image.png|600]] — pothole is "600"
        let src = "![[image.png|600]]";
        let refs = extract_md_references(src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "image.png");
        assert!(matches!(&refs[0].syntax, RefSyntax::WikilinkAliasedEmbed { display } if display == "600"));
    }
}
