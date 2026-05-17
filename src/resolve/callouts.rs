/// Transform Obsidian callout syntax into HTML divs.
///
/// Callout syntax:
/// ```markdown
/// > [!warning] Be Careful
/// > This is a warning callout.
/// > It can span multiple lines.
/// ```
///
/// Produces:
/// ```html
/// <div class="callout" data-type="warning">
///   <div class="callout-title">Be Careful</div>
///   <div class="callout-content">
/// This is a warning callout.
/// It can span multiple lines.
///   </div>
/// </div>
/// ```
///
/// Content inside fenced code blocks is left untouched.
/// Regular blockquotes (no `[!type]` marker) are left unchanged.
pub fn transform_callouts(content: &str) -> String {
    const SUPPORTED_TYPES: &[&str] = &[
        "note", "tip", "warning", "caution", "important", "info", "abstract", "todo", "success",
        "question", "failure", "danger", "bug", "example", "quote", "pending",
    ];

    let lines: Vec<&str> = content.lines().collect();
    let mut output = String::with_capacity(content.len());
    let mut i = 0;
    let mut in_code_block = false;
    let mut fence_char = ' ';

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // --- Fenced code block tracking (matches wikilinks.rs pattern) ---
        if in_code_block {
            let closes = trimmed.starts_with(fence_char)
                && trimmed.chars().take(3).all(|c| c == fence_char)
                && trimmed.trim_matches(fence_char).trim().is_empty();
            if closes {
                in_code_block = false;
            }
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        let fence_rest = trimmed
            .strip_prefix("```")
            .map(|r| ('`', r))
            .or_else(|| trimmed.strip_prefix("~~~").map(|r| ('~', r)));
        if let Some((candidate_char, rest)) = fence_rest {
            if !rest.contains(candidate_char) {
                fence_char = candidate_char;
                in_code_block = true;
                output.push_str(line);
                output.push('\n');
                i += 1;
                continue;
            }
        }

        // Check whether this line is a callout header: `> [!type]` or `> [!type] Title`
        if let Some((callout_type, title)) = parse_callout_header(line, SUPPORTED_TYPES) {
            // Collect all continuation lines that belong to this callout block.
            // A continuation line starts with `>` (possibly with trailing space / empty after `>`).
            let mut body_lines: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                // Strip the leading `> ` or `>` prefix (both ASCII).
                let stripped = if let Some(rest) = next.strip_prefix("> ") {
                    Some(rest)
                } else if next == ">" {
                    Some("")
                } else {
                    // `>text` with no space — strip just the `>`
                    next.strip_prefix('>')
                };
                if let Some(s) = stripped {
                    body_lines.push(s);
                    i += 1;
                } else {
                    break;
                }
            }

            // Determine the display title.
            let display_title = if title.is_empty() {
                capitalize(&callout_type)
            } else {
                title
            };

            // Build the body content, then recurse so that a nested `> [!type]`
            // inside the body (one `> ` shallower after stripping) is itself
            // transformed into a callout div. The docs promise nested callouts.
            let body = transform_callouts(&body_lines.join("\n"));
            let body = body.trim_end_matches('\n').to_string();

            let escaped_title = html_escape(&display_title);
            // CommonMark requires blank lines between an HTML block tag and
            // embedded markdown content, otherwise the renderer treats the
            // body as raw HTML and does not process inline markup. Emit a
            // blank line before and after {body}.
            //
            // The closing </div>s are kept at column 0 (not indented) so
            // markdown lists in the body close cleanly — any leading indent
            // on the closer would be interpreted as list-item continuation
            // and pull the tag inside the final <li>.
            output.push_str(&format!(
                "<div class=\"callout\" data-type=\"{callout_type}\">\n  <div class=\"callout-title\">{escaped_title}</div>\n  <div class=\"callout-content\">\n\n{body}\n\n</div>\n</div>\n"
            ));
        } else {
            output.push_str(line);
            output.push('\n');
            i += 1;
        }
    }

    // Remove a single trailing newline that we unconditionally added, if the
    // original content did not end with a newline.
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }

    output
}

/// Try to parse a callout header from a single line.
///
/// Returns `Some((type_lowercase, title_string))` when the line matches
/// `> [!SupportedType]` or `> [!SupportedType] Some Title`, `None` otherwise.
fn parse_callout_header(line: &str, supported_types: &[&str]) -> Option<(String, String)> {
    // Must start with `> `
    let rest = line.strip_prefix("> ")?;

    // Must be followed by `[!`
    let rest = rest.strip_prefix("[!")?;

    // Find the closing `]` and split around it.
    let (raw_type, after_bracket) = rest.split_once(']')?;

    let type_lower = raw_type.to_lowercase();
    if !supported_types.contains(&type_lower.as_str()) {
        return None;
    }

    // Optional title after the `]`: either nothing or ` Title text`
    let title = if after_bracket.is_empty() {
        String::new()
    } else if let Some(t) = after_bracket.strip_prefix(' ') {
        t.to_string()
    } else {
        // Unexpected format — not a recognised callout header.
        return None;
    };

    Some((type_lower, title))
}

use crate::media::html_escape;

/// Capitalise the first character of a string.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: strip trailing newlines for comparison so tests are insensitive to
    /// a single trailing newline difference.
    fn norm(s: &str) -> &str {
        s.trim_end_matches('\n')
    }

    #[test]
    fn test_basic_callout() {
        let input = "> [!warning] Be Careful\n> This is a warning callout.\n> It can span multiple lines.";
        let output = transform_callouts(input);
        let expected = "<div class=\"callout\" data-type=\"warning\">\n  <div class=\"callout-title\">Be Careful</div>\n  <div class=\"callout-content\">\n\nThis is a warning callout.\nIt can span multiple lines.\n\n</div>\n</div>";
        assert_eq!(norm(&output), expected);
    }

    #[test]
    fn test_callout_no_title() {
        let input = "> [!warning]\n> Watch out!";
        let output = transform_callouts(input);
        assert!(output.contains("<div class=\"callout-title\">Warning</div>"));
        assert!(output.contains(r#"data-type="warning""#));
    }

    #[test]
    fn test_all_callout_types() {
        let types = [
            "note", "tip", "warning", "caution", "important", "info", "abstract", "todo",
            "success", "question", "failure", "danger", "bug", "example", "quote",
        ];
        for t in &types {
            let input = format!("> [!{t}]\n> Body text.");
            let output = transform_callouts(&input);
            assert!(
                output.contains(&format!(r#"data-type="{t}""#)),
                "type `{t}` not recognised"
            );
        }
    }

    #[test]
    fn test_callout_in_code_block_preserved() {
        let input = "```\n> [!warning] Be Careful\n> This should not be transformed.\n```";
        let output = transform_callouts(input);
        assert_eq!(norm(&output), norm(input));
    }

    #[test]
    fn test_regular_blockquote_unchanged() {
        let input = "> This is a regular blockquote.\n> It should not be transformed.";
        let output = transform_callouts(input);
        assert_eq!(norm(&output), norm(input));
    }

    #[test]
    fn test_multi_paragraph_callout() {
        // An empty `>` line separates paragraphs inside the callout body.
        let input = "> [!info] Multi\n> First paragraph.\n>\n> Second paragraph.";
        let output = transform_callouts(input);
        assert!(output.contains(r#"data-type="info""#));
        assert!(output.contains("callout-title"));
        // Both paragraphs should appear inside callout-content.
        assert!(output.contains("First paragraph."));
        assert!(output.contains("Second paragraph."));
    }

    #[test]
    fn test_case_insensitive_type() {
        let input = "> [!WARNING] Loud Warning\n> This still works.";
        let output = transform_callouts(input);
        // The data-type attribute must use the lowercase form.
        assert!(output.contains(r#"data-type="warning""#));
        assert!(!output.contains(r#"data-type="WARNING""#));
        assert!(output.contains("Loud Warning"));
    }

    #[test]
    fn test_multiple_callouts() {
        let input = "> [!note] First\n> First body.\n\n> [!tip] Second\n> Second body.";
        let output = transform_callouts(input);
        assert!(output.contains(r#"data-type="note""#));
        assert!(output.contains(r#"data-type="tip""#));
        assert!(output.contains("First body."));
        assert!(output.contains("Second body."));
    }

    #[test]
    fn test_callout_followed_by_text() {
        let input = "> [!note] A Note\n> Note body.\n\nSome paragraph after the callout.";
        let output = transform_callouts(input);
        assert!(output.contains(r#"data-type="note""#));
        assert!(output.contains("Some paragraph after the callout."));
    }

    #[test]
    fn test_empty_callout() {
        // Callout header with no continuation lines.
        let input = "> [!tip] No Body";
        let output = transform_callouts(input);
        assert!(output.contains(r#"data-type="tip""#));
        assert!(output.contains("<div class=\"callout-title\">No Body</div>"));
        // Body section should exist but be empty (just whitespace/newlines).
        assert!(output.contains("<div class=\"callout-content\">"));
    }

    #[test]
    fn test_callout_title_html_escaped() {
        // HTML special characters in the title must be escaped to prevent XSS.
        let input = "> [!warning] Use <script> & \"quotes\"\n> Body text.";
        let output = transform_callouts(input);
        assert!(
            output.contains("Use &lt;script&gt; &amp; &quot;quotes&quot;"),
            "HTML special chars in title must be escaped, got: {}",
            output,
        );
        // Must NOT contain the raw unescaped characters in the title div.
        assert!(!output.contains("<div class=\"callout-title\">Use <script>"));
    }

    #[test]
    fn test_html_escape_helper() {
        assert_eq!(html_escape("safe text"), "safe text");
        assert_eq!(html_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
        assert_eq!(html_escape("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(
            html_escape("<script>alert(\"xss\");</script>"),
            "&lt;script&gt;alert(&quot;xss&quot;);&lt;/script&gt;"
        );
    }

    #[test]
    fn test_nested_callouts_recognized() {
        // The docs promise nested callouts. Inside a callout body (after one
        // `> ` is stripped from each line), another `> [!type]` header must
        // itself be transformed into a callout div — not left as a plain
        // <blockquote> with literal `[!type]` text.
        let input = "> [!warning] Outer\n> Outer content.\n>\n> > [!tip] Inner\n> > Inner content.";
        let output = transform_callouts(input);
        assert!(output.contains("callout-warning"), "outer callout missing: {}", output);
        assert!(output.contains("Outer content."), "outer body missing: {}", output);
        assert!(output.contains("callout-tip"), "inner callout not recognized: {}", output);
        assert!(output.contains("Inner content."), "inner body missing: {}", output);
        // The `[!tip]` marker must not survive as literal text.
        assert!(!output.contains("[!tip]"), "inner [!tip] marker leaked: {}", output);
    }

    #[test]
    fn test_closing_div_not_indented_so_markdown_lists_close_cleanly() {
        // If the closing `</div>` is indented (e.g., "  </div>"), CommonMark
        // treats the two-space indent as list-item continuation, and the
        // closing tag gets pulled INSIDE the last <li>. Keeping the closers
        // at column 0 prevents that.
        let input = "> [!tip] List inside\n> - one\n> - two";
        let output = transform_callouts(input);
        assert!(
            output.contains("\n</div>\n</div>"),
            "closing </div>s must not be indented (would bleed into adjacent markdown lists), got: {}",
            output,
        );
    }

    #[test]
    fn test_body_is_separated_from_wrapper_divs_for_markdown_parsing() {
        // CommonMark rule: content immediately adjacent to an HTML block tag is
        // treated as raw HTML — inline markdown inside is NOT parsed. To get
        // `**bold**` inside a callout's first paragraph rendered as <strong>,
        // the body must be separated from the surrounding <div> tags by blank
        // lines so the markdown renderer treats it as its own markdown block.
        //
        // This test locks in that contract on the emitted string. Removing the
        // blank lines would reintroduce the "first-paragraph inline markdown
        // does not render" bug.
        let input = "> [!note] Title\n> Body with **bold**.";
        let output = transform_callouts(input);
        // Blank line between <div class="callout-content"> and the body:
        assert!(
            output.contains("<div class=\"callout-content\">\n\nBody with **bold**."),
            "expected blank line after callout-content opener, got: {}",
            output,
        );
        // Blank line between the body and the closing </div>:
        assert!(
            output.contains("Body with **bold**.\n\n</div>"),
            "expected blank line before callout-content closer, got: {}",
            output,
        );
    }

    #[test]
    fn test_pending_callout_type() {
        let input = "> [!pending] Trailer video\n> Add when ready.";
        let result = transform_callouts(input);
        assert!(result.contains(r#"<div class="callout callout-pending">"#),
            "Expected callout-pending class. Got: {}", result);
        assert!(result.contains("Trailer video"), "Expected title");
        assert!(result.contains("Add when ready."), "Expected body");
    }
}
