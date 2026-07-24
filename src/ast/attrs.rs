//! Attribute-block parser for the unified shortcode grammar.
//!
//! Parses the `{ ... }` portion of an opening fence into structured
//! classes, an optional id, and key/value pairs. Pure, no I/O.
//!
//! Grammar (from `docs/archive/2026-05-02-shortcode-grammar-design.md`):
//!
//! ```text
//! Attrs    := "{" AttrItem (whitespace AttrItem)* "}"
//! AttrItem := "." classname
//!           | "#" id
//!           | key "=" value
//! key      := [A-Za-z][A-Za-z0-9_-]*
//! value    := bareword | quoted
//! bareword := [A-Za-z0-9:_/.\-]+
//! quoted   := "\"" any-char-except-unescaped-quote* "\""
//! ```
//!
//! Whitespace inside `{}` (spaces, tabs, newlines) all separate items
//! identically — multi-line attribute blocks are first-class.
//!
//! The parser is forgiving on malformed bareword values (returns the
//! malformed token verbatim rather than erroring); it errors only on
//! structural problems (unterminated quote, bad key, no closing brace).
//! Renderers are responsible for validating typed values like `cols=int`.

/// Parsed attribute block.
///
/// `kvs` is a `Vec<(String, String)>` (not a `HashMap`) so iteration is
/// stable and matches source order — important for deterministic HTML
/// attribute output, snapshot tests, and diagnostics that point at the
/// offending entry. Last-write-wins on duplicate keys is enforced in the
/// parser, so callers can use [`AttrBlock::get`] without seeing stale
/// values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttrBlock {
    /// `.classname` shortcuts, in source order. Duplicates preserved so
    /// the renderer can detect explicit doubles if it wants.
    pub classes: Vec<String>,
    /// Last `#id` shortcut wins (multiple ids is malformed but not fatal).
    pub id: Option<String>,
    /// `key=value` pairs in source order. Last write wins when a key
    /// repeats — the parser drops earlier entries on collision.
    pub kvs: Vec<(String, String)>,
    /// Width flag (spec § P9): `body | wide | page | screen`. Recognized
    /// as a bare token in the attribute block; `{full}` is normalized to
    /// `"screen"`. Last write wins on repeats. `None` means the author did
    /// not specify a width — emitters should omit `data-width` in that
    /// case so the HTML stays sparse and themes can target the absence.
    pub width: Option<&'static str>,
}

impl AttrBlock {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
            && self.id.is_none()
            && self.kvs.is_empty()
            && self.width.is_none()
    }

    /// Convenience for renderers: get the value for a key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kvs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Space-joined class list, ready for `class="..."`.
    pub fn class_string(&self) -> String {
        self.classes.join(" ")
    }

    /// Set a key/value pair, replacing any existing entry for `key`
    /// in place to preserve source order of unrelated entries.
    fn set_kv(&mut self, key: String, value: String) {
        if let Some(slot) = self.kvs.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = value;
        } else {
            self.kvs.push((key, value));
        }
    }
}

/// Errors parsing an attribute block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrError {
    /// Input did not start with `{`.
    MissingOpenBrace,
    /// Block opened with `{` but no matching `}` was found.
    UnclosedBrace,
    /// A `"` opened but had no closing `"` before the block ended.
    UnterminatedQuote,
    /// A `key=` was followed by no value (end of input or whitespace).
    EmptyValue { key: String },
    /// A token started with `=` (no key before it) or had a malformed key.
    InvalidKey { token: String },
}

/// Parse an attribute block.
///
/// `input` must include the surrounding braces: `{.foo key=bar}`.
/// Whitespace between items (including newlines) is permitted.
///
/// Errors only on structural problems. Unrecognized characters at the start
/// of an item — anything not `.`, `#`, or a valid key character — produce
/// `InvalidKey { token }` so the caller can surface a useful diagnostic.
pub fn parse_attrs(input: &str) -> Result<AttrBlock, AttrError> {
    let mut chars = input.char_indices().peekable();

    // Expect leading `{`.
    skip_ws(&mut chars);
    match chars.next() {
        Some((_, '{')) => {}
        _ => return Err(AttrError::MissingOpenBrace),
    }

    let mut block = AttrBlock::default();

    loop {
        skip_ws(&mut chars);
        match chars.peek().copied() {
            None => return Err(AttrError::UnclosedBrace),
            Some((_, '}')) => {
                chars.next();
                return Ok(block);
            }
            Some((_, '.')) => {
                chars.next();
                let class = read_bareword(&mut chars);
                if !class.is_empty() {
                    block.classes.push(class);
                }
            }
            Some((_, '#')) => {
                chars.next();
                let id = read_bareword(&mut chars);
                if !id.is_empty() {
                    block.id = Some(id);
                }
            }
            Some((_, c)) if is_key_start(c) => {
                let key = read_key(&mut chars);
                skip_ws_inline(&mut chars);
                match chars.peek().copied() {
                    Some((_, '=')) => {
                        chars.next();
                        skip_ws_inline(&mut chars);
                        let value = read_value(&mut chars, &key)?;
                        block.set_kv(key, value);
                    }
                    _ => {
                        // Bare keyword (no `=value`). Spec § P9 reserves four
                        // width tokens (`body | wide | page | screen`) plus
                        // the alias `full` (→ `screen`) as bare flags for
                        // hero / gallery / grid / embed / image-wrapper
                        // sizing. Any other bare keyword is still an error.
                        if let Some(width) = match_width_token(&key) {
                            block.width = Some(width);
                        } else {
                            return Err(AttrError::InvalidKey { token: key });
                        }
                    }
                }
            }
            Some((_, c)) => {
                // Anything else at item-start is invalid. Capture the run
                // up to the next whitespace/`}` for the diagnostic.
                let mut token = String::new();
                token.push(c);
                chars.next();
                while let Some(&(_, ch)) = chars.peek() {
                    if ch.is_whitespace() || ch == '}' {
                        break;
                    }
                    token.push(ch);
                    chars.next();
                }
                return Err(AttrError::InvalidKey { token });
            }
        }
    }
}

fn is_key_start(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// Recognize the spec § P9 width tokens (`body | wide | page | screen | full`).
///
/// `full` is the author-facing alias for `screen` (the spec keeps both shapes
/// because `{full}` reads naturally in authoring contexts while the emitted
/// attribute value is the value-space term `screen`). All five tokens are
/// ASCII lowercase per the grammar; the parser feeds keys verbatim, so any
/// case-folding decision lives here.
fn match_width_token(s: &str) -> Option<&'static str> {
    match s {
        "body" => Some("body"),
        "wide" => Some("wide"),
        "page" => Some("page"),
        "screen" | "full" => Some("screen"),
        _ => None,
    }
}

fn is_key_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_bareword(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z' | '0'..='9' |
        ':' | '/' | '.' | '-' | '_'
    )
}

fn skip_ws<I>(iter: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = (usize, char)>,
{
    while let Some(&(_, c)) = iter.peek() {
        if c.is_whitespace() {
            iter.next();
        } else {
            break;
        }
    }
}

/// Like `skip_ws` but stops at the first newline — used after a `key`
/// before checking for `=`. Newline-after-key without `=` is a malformed
/// item, but the user might have written `key\n=value` which we should
/// accept (whitespace is whitespace inside `{}`). Today we treat all
/// whitespace identically — so this is currently equivalent to skip_ws.
/// Kept as a separate helper in case future grammars distinguish.
fn skip_ws_inline<I>(iter: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = (usize, char)>,
{
    skip_ws(iter);
}

fn read_bareword<I>(iter: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = (usize, char)>,
{
    let mut s = String::new();
    while let Some(&(_, c)) = iter.peek() {
        if is_bareword(c) {
            s.push(c);
            iter.next();
        } else {
            break;
        }
    }
    s
}

fn read_key<I>(iter: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = (usize, char)>,
{
    let mut s = String::new();
    if let Some(&(_, c)) = iter.peek() {
        if is_key_start(c) {
            s.push(c);
            iter.next();
        } else {
            return s;
        }
    }
    while let Some(&(_, c)) = iter.peek() {
        if is_key_continue(c) {
            s.push(c);
            iter.next();
        } else {
            break;
        }
    }
    s
}

fn read_value<I>(iter: &mut std::iter::Peekable<I>, key: &str) -> Result<String, AttrError>
where
    I: Iterator<Item = (usize, char)>,
{
    match iter.peek().copied() {
        Some((_, '"')) => {
            iter.next();
            read_quoted(iter)
        }
        Some((_, c)) if is_bareword(c) => Ok(read_bareword(iter)),
        _ => Err(AttrError::EmptyValue { key: key.to_string() }),
    }
}

/// Read until the closing `"`, supporting `\"` and `\\` escapes.
fn read_quoted<I>(iter: &mut std::iter::Peekable<I>) -> Result<String, AttrError>
where
    I: Iterator<Item = (usize, char)>,
{
    let mut s = String::new();
    loop {
        match iter.next() {
            None => return Err(AttrError::UnterminatedQuote),
            Some((_, '"')) => return Ok(s),
            Some((_, '\\')) => match iter.next() {
                None => return Err(AttrError::UnterminatedQuote),
                Some((_, ch)) => s.push(ch),
            },
            Some((_, ch)) => s.push(ch),
        }
    }
}

// ── Multi-line opener support ────────────────────────────────────────
//
// The shortcode extractor needs to know when an opener line's `{`
// doesn't close on the same line, so it can absorb subsequent lines
// into the attribute block before parsing. These helpers live here
// (next to `parse_attrs`) because they're grammar primitives, not
// extraction helpers — any future consumer that sees a partial attr
// block (e.g. an editor decoration that wants to highlight the live
// state of a fenced div as the user types) needs them too.

/// Quote-aware brace-depth tracker. Returns the depth after consuming
/// `s`, starting from `start_depth`.
///
/// Tracks `"`-quoted strings so that `{` and `}` inside a value like
/// `key="{name}"` don't shift the depth. Backslash escapes inside a
/// string consume the next character verbatim.
pub fn brace_depth(s: &str, start_depth: i32) -> i32 {
    let mut depth = start_depth;
    let mut in_quote = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if in_quote {
            match c {
                '"' => in_quote = false,
                '\\' => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_quote = true,
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    depth
}

/// If `single_line_args` opens a `{` that doesn't close on the same line,
/// scan `following_lines` and join them onto args until the brace closes.
///
/// Returns `(extended_args_owned, lines_consumed)` where:
/// - `extended_args_owned = Some(s)` when multi-line scanning happened.
///   `None` means the single-line args already balance and the caller
///   should keep using the original `&str`.
/// - `lines_consumed` is how many lines after the opener were absorbed
///   into the attribute block. The body starts at
///   `following_lines[lines_consumed..]`.
///
/// Brace balancing tracks ASCII `{` and `}` outside quoted strings. A
/// `\"`-escape inside a quoted string is recognized so authored content
/// like `key="say \"hi\""` doesn't desynchronize the scanner.
///
/// If the brace never closes (ill-formed input), returns the gathered
/// content verbatim so the caller can pass it on; the attribute parser
/// will emit a structural error and the block falls through to the
/// pass-through path.
pub fn gather_multi_line_attrs(
    single_line_args: &str,
    following_lines: &[&str],
) -> (Option<String>, usize) {
    let depth_after_first = brace_depth(single_line_args, 0);
    if depth_after_first == 0 {
        return (None, 0);
    }

    let mut combined = single_line_args.to_string();
    let mut depth = depth_after_first;
    let mut consumed = 0;
    for &line in following_lines {
        // Insert a newline so the attribute parser sees the line break
        // as whitespace (its grammar treats all whitespace identically).
        combined.push('\n');
        combined.push_str(line);
        consumed += 1;
        depth = brace_depth(line, depth);
        if depth == 0 {
            return (Some(combined), consumed);
        }
    }

    // Brace never closed within the document.
    (Some(combined), consumed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> AttrBlock {
        parse_attrs(s).expect("parse_attrs should succeed")
    }

    fn err(s: &str) -> AttrError {
        parse_attrs(s).expect_err("parse_attrs should fail")
    }

    // ── empty / no-content cases ─────────────────────────────────────

    #[test]
    fn empty_block() {
        let b = ok("{}");
        assert!(b.is_empty());
    }

    #[test]
    fn whitespace_only_block() {
        let b = ok("{   }");
        assert!(b.is_empty());
    }

    #[test]
    fn missing_open_brace_errors() {
        assert_eq!(err("foo=bar"), AttrError::MissingOpenBrace);
    }

    #[test]
    fn unclosed_brace_errors() {
        assert_eq!(err("{.foo"), AttrError::UnclosedBrace);
    }

    // ── classes ──────────────────────────────────────────────────────

    #[test]
    fn single_class() {
        let b = ok("{.primary}");
        assert_eq!(b.classes, vec!["primary"]);
        assert_eq!(b.class_string(), "primary");
    }

    #[test]
    fn multiple_classes_space_separated() {
        let b = ok("{.primary .large}");
        assert_eq!(b.classes, vec!["primary", "large"]);
        assert_eq!(b.class_string(), "primary large");
    }

    #[test]
    fn classes_preserve_source_order() {
        let b = ok("{.first .second .third}");
        assert_eq!(b.classes, vec!["first", "second", "third"]);
    }

    #[test]
    fn class_with_dash_and_digit() {
        let b = ok("{.btn-primary .v2}");
        assert_eq!(b.classes, vec!["btn-primary", "v2"]);
    }

    #[test]
    fn dot_with_no_name_skipped() {
        // Lonely `.` followed by whitespace produces an empty class — drop it
        // rather than insert "" into the list.
        let b = ok("{. .real}");
        assert_eq!(b.classes, vec!["real"]);
    }

    // ── id ───────────────────────────────────────────────────────────

    #[test]
    fn single_id() {
        let b = ok("{#hero}");
        assert_eq!(b.id.as_deref(), Some("hero"));
    }

    #[test]
    fn multiple_ids_last_wins() {
        let b = ok("{#first #second}");
        assert_eq!(b.id.as_deref(), Some("second"));
    }

    #[test]
    fn id_and_class_in_same_block() {
        let b = ok("{#main .container}");
        assert_eq!(b.id.as_deref(), Some("main"));
        assert_eq!(b.classes, vec!["container"]);
    }

    // ── key/value: bareword values ──────────────────────────────────

    #[test]
    fn key_with_bareword_integer() {
        let b = ok("{cols=3}");
        assert_eq!(b.get("cols"), Some("3"));
    }

    #[test]
    fn key_with_ratio_value() {
        let b = ok("{cols=1:1:2}");
        assert_eq!(b.get("cols"), Some("1:1:2"));
    }

    #[test]
    fn key_with_path_value() {
        let b = ok("{image=hero.jpg}");
        assert_eq!(b.get("image"), Some("hero.jpg"));
    }

    #[test]
    fn key_with_dotted_path_value() {
        let b = ok("{image=path/to/file.jpg}");
        assert_eq!(b.get("image"), Some("path/to/file.jpg"));
    }

    #[test]
    fn key_with_negative_int_value() {
        let b = ok("{offset=-5}");
        assert_eq!(b.get("offset"), Some("-5"));
    }

    #[test]
    fn multiple_kvs() {
        let b = ok("{cols=3 image=hero.jpg gap=2}");
        assert_eq!(b.get("cols"), Some("3"));
        assert_eq!(b.get("image"), Some("hero.jpg"));
        assert_eq!(b.get("gap"), Some("2"));
    }

    #[test]
    fn duplicate_key_last_wins() {
        let b = ok("{cols=2 cols=4}");
        assert_eq!(b.get("cols"), Some("4"));
    }

    #[test]
    fn key_with_dash_and_underscore() {
        let b = ok("{button-text=foo my_field=bar}");
        assert_eq!(b.get("button-text"), Some("foo"));
        assert_eq!(b.get("my_field"), Some("bar"));
    }

    // ── key/value: quoted values ────────────────────────────────────

    #[test]
    fn key_with_quoted_simple() {
        let b = ok(r#"{button="Request access"}"#);
        assert_eq!(b.get("button"), Some("Request access"));
    }

    #[test]
    fn key_with_quoted_punctuation() {
        let b = ok(r#"{description="One email. No newsletter."}"#);
        assert_eq!(b.get("description"), Some("One email. No newsletter."));
    }

    #[test]
    fn quoted_value_with_escaped_quote() {
        let b = ok(r#"{label="say \"hi\""}"#);
        assert_eq!(b.get("label"), Some(r#"say "hi""#));
    }

    #[test]
    fn quoted_value_with_escaped_backslash() {
        let b = ok(r#"{path="C:\\Users\\me"}"#);
        assert_eq!(b.get("path"), Some(r"C:\Users\me"));
    }

    #[test]
    fn quoted_value_with_brace_inside() {
        // The closing `}` of the block must NOT be confused with a `}`
        // inside a quoted value.
        let b = ok(r#"{template="{name}"}"#);
        assert_eq!(b.get("template"), Some("{name}"));
    }

    #[test]
    fn quoted_value_can_be_empty() {
        let b = ok(r#"{label=""}"#);
        assert_eq!(b.get("label"), Some(""));
    }

    #[test]
    fn unterminated_quote_errors() {
        assert_eq!(
            err(r#"{button="oops}"#),
            AttrError::UnterminatedQuote
        );
    }

    // ── multi-line ───────────────────────────────────────────────────

    #[test]
    fn multi_line_attrs() {
        let b = ok("{\n  placeholder=\"you@domain.com\"\n  button=\"Request access\"\n}");
        assert_eq!(b.get("placeholder"), Some("you@domain.com"));
        assert_eq!(b.get("button"), Some("Request access"));
    }

    #[test]
    fn multi_line_with_classes_and_kvs() {
        let b = ok("{\n  .primary\n  .large\n  cols=3\n  image=hero.jpg\n}");
        assert_eq!(b.classes, vec!["primary", "large"]);
        assert_eq!(b.get("cols"), Some("3"));
        assert_eq!(b.get("image"), Some("hero.jpg"));
    }

    #[test]
    fn tab_separator_works() {
        let b = ok("{.foo\tcols=3}");
        assert_eq!(b.classes, vec!["foo"]);
        assert_eq!(b.get("cols"), Some("3"));
    }

    // ── error paths ──────────────────────────────────────────────────

    #[test]
    fn key_with_no_value_errors() {
        let e = err("{cols=}");
        assert!(matches!(e, AttrError::EmptyValue { ref key } if key == "cols"));
    }

    #[test]
    fn key_without_equals_errors() {
        // A bare keyword without `=` is invalid (spec says only `.foo`,
        // `#foo`, `key=value` are recognized) — EXCEPT for the spec § P9
        // width tokens (body | wide | page | screen | full), which are
        // tested separately below.
        assert!(matches!(err("{cols}"), AttrError::InvalidKey { .. }));
        assert!(matches!(err("{flush}"), AttrError::InvalidKey { .. }));
    }

    // ── width tokens (spec § P9) ─────────────────────────────────────

    #[test]
    fn width_token_body() {
        let b = ok("{body}");
        assert_eq!(b.width, Some("body"));
        assert!(b.classes.is_empty());
        assert!(b.id.is_none());
        assert!(b.kvs.is_empty());
    }

    #[test]
    fn width_token_wide() {
        let b = ok("{wide}");
        assert_eq!(b.width, Some("wide"));
    }

    #[test]
    fn width_token_page() {
        let b = ok("{page}");
        assert_eq!(b.width, Some("page"));
    }

    #[test]
    fn width_token_screen() {
        let b = ok("{screen}");
        assert_eq!(b.width, Some("screen"));
    }

    #[test]
    fn width_token_full_aliases_to_screen() {
        // Per spec § P9 authoring grammar: `full` is the author-facing
        // alias for `screen`. The emitted value is always the value-space
        // term `screen`.
        let b = ok("{full}");
        assert_eq!(b.width, Some("screen"));
    }

    #[test]
    fn width_token_with_class() {
        let b = ok("{wide .showcase}");
        assert_eq!(b.width, Some("wide"));
        assert_eq!(b.classes, vec!["showcase"]);
    }

    #[test]
    fn width_token_with_kv() {
        let b = ok("{cols=3 wide}");
        assert_eq!(b.width, Some("wide"));
        assert_eq!(b.get("cols"), Some("3"));
    }

    #[test]
    fn width_token_repeated_last_wins() {
        // Authors are unlikely to do this, but stay deterministic.
        let b = ok("{wide page}");
        assert_eq!(b.width, Some("page"));
    }

    #[test]
    fn width_token_does_not_become_class() {
        let b = ok("{wide}");
        assert!(b.classes.is_empty());
    }

    #[test]
    fn width_token_with_explicit_dot_is_a_class_not_width() {
        // `.wide` is still a class — width tokens are recognized only as
        // bare keywords, not as `.class` shortcuts.
        let b = ok("{.wide}");
        assert_eq!(b.classes, vec!["wide"]);
        assert!(b.width.is_none());
    }

    #[test]
    fn key_starting_with_digit_is_invalid_token() {
        let e = err("{3cols=2}");
        assert!(matches!(e, AttrError::InvalidKey { .. }));
    }

    #[test]
    fn lonely_equals_is_invalid_token() {
        let e = err("{=foo}");
        assert!(matches!(e, AttrError::InvalidKey { .. }));
    }

    // ── round-trip via class_string / get ────────────────────────────

    #[test]
    fn class_string_joins_with_spaces() {
        let b = ok("{.alpha .beta .gamma}");
        assert_eq!(b.class_string(), "alpha beta gamma");
    }

    #[test]
    fn class_string_empty_when_no_classes() {
        let b = ok("{cols=3}");
        assert_eq!(b.class_string(), "");
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let b = ok("{cols=3}");
        assert!(b.get("rows").is_none());
    }

    // ── realistic spec examples ──────────────────────────────────────

    #[test]
    fn spec_subscribe_form_multi_line() {
        let b = ok("{\n  placeholder=\"you@domain.com\"\n  button=\"Request access\"\n}");
        assert_eq!(b.get("placeholder"), Some("you@domain.com"));
        assert_eq!(b.get("button"), Some("Request access"));
        assert!(b.classes.is_empty());
        assert!(b.id.is_none());
    }

    #[test]
    fn spec_buttons_classes_only() {
        let b = ok("{.primary .large}");
        assert_eq!(b.classes, vec!["primary", "large"]);
        assert!(b.kvs.is_empty());
    }

    #[test]
    fn spec_gallery_cols_and_class() {
        let b = ok("{cols=3 .showcase}");
        assert_eq!(b.get("cols"), Some("3"));
        assert_eq!(b.classes, vec!["showcase"]);
    }

    #[test]
    fn spec_grid_ratio_cols() {
        let b = ok("{cols=1:1:2}");
        assert_eq!(b.get("cols"), Some("1:1:2"));
    }

    #[test]
    fn spec_hero_image_path() {
        let b = ok("{image=hero.jpg}");
        assert_eq!(b.get("image"), Some("hero.jpg"));
    }

    #[test]
    fn spec_pure_css_region_class_and_id() {
        let b = ok("{.tagline #intro}");
        assert_eq!(b.classes, vec!["tagline"]);
        assert_eq!(b.id.as_deref(), Some("intro"));
    }

    // ── leading whitespace before brace ──────────────────────────────

    #[test]
    fn leading_whitespace_before_brace_ok() {
        // The opener-scanner upstream may pass " {.foo}" if it strips name
        // first. Tolerate leading whitespace.
        let b = ok("  {.foo}");
        assert_eq!(b.classes, vec!["foo"]);
    }

    // ── brace-depth tracker ──────────────────────────────────────────

    #[test]
    fn brace_depth_tracks_simple_open_close() {
        assert_eq!(brace_depth("{}", 0), 0);
        assert_eq!(brace_depth("{", 0), 1);
        assert_eq!(brace_depth("}", 1), 0);
    }

    #[test]
    fn brace_depth_ignores_braces_in_quoted_strings() {
        assert_eq!(brace_depth(r#"{key="{name}"}"#, 0), 0);
        assert_eq!(brace_depth(r#"{key="{"}"#, 0), 0);
    }

    #[test]
    fn brace_depth_handles_escaped_quote() {
        assert_eq!(brace_depth(r#"{label="say \"hi\""}"#, 0), 0);
    }

    #[test]
    fn brace_depth_clamps_at_zero_for_orphan_close() {
        // A stray `}` with no matching `{` shouldn't go negative —
        // depth-0 input followed by `}` stays 0.
        assert_eq!(brace_depth("}", 0), 0);
    }

    // ── multi-line attr gather ───────────────────────────────────────

    #[test]
    fn gather_no_brace_returns_none_zero_consumed() {
        let (out, consumed) = gather_multi_line_attrs("plain text", &["following"]);
        assert!(out.is_none());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn gather_balanced_single_line_returns_none() {
        let (out, consumed) = gather_multi_line_attrs("{.foo}", &["body"]);
        assert!(out.is_none());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn gather_two_line_block_returns_combined() {
        let (out, consumed) = gather_multi_line_attrs("{", &[".foo", "}", "body"]);
        assert_eq!(consumed, 2);
        let combined = out.expect("multi-line should return a combined string");
        let parsed = parse_attrs(&combined).expect("combined attrs should parse");
        assert_eq!(parsed.classes, vec!["foo"]);
    }

    #[test]
    fn gather_three_line_block_with_kvs() {
        let (out, consumed) = gather_multi_line_attrs(
            "{",
            &["  placeholder=\"you@domain.com\"", "  button=\"Go\"", "}", "body"],
        );
        assert_eq!(consumed, 3);
        let parsed = parse_attrs(&out.unwrap()).unwrap();
        assert_eq!(parsed.get("placeholder"), Some("you@domain.com"));
        assert_eq!(parsed.get("button"), Some("Go"));
    }

    #[test]
    fn gather_unclosed_returns_what_it_has() {
        let (out, consumed) = gather_multi_line_attrs("{", &[".foo", ".bar"]);
        // Both lines were consumed, brace never closed.
        assert_eq!(consumed, 2);
        // The combined string is returned even though it's unparseable.
        assert!(out.is_some());
    }

    #[test]
    fn gather_quoted_brace_does_not_count() {
        // A `}` inside a quoted value must not close the block prematurely.
        let (out, consumed) = gather_multi_line_attrs(
            "{",
            &[r#"  template="say }"#, r#"  end"#, "}"],
        );
        // Three lines absorbed (the closing brace is on the third).
        assert_eq!(consumed, 3);
        assert!(out.is_some());
    }
}
