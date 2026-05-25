//! Title-attribute params grammar for moss-extension syntax.
//!
//! Stage 1 emits `moss:width=wide align=left` into the markdown image/link
//! title attribute. Stage 2 reads the title attribute, checks for the
//! `moss:` prefix, parses the params back.
//!
//! Grammar:
//!   title       := "moss:" params
//!   params      := param ( WS param )*
//!   param       := key "=" value
//!   key         := /[a-z][a-z0-9_-]*/
//!   value       := bare_value | "\"" quoted_chars "\""
//!   bare_value  := /[^\s"]+/
//!   quoted_chars := /([^"\\]|\\.)*/

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TitleParams {
    pub params: BTreeMap<String, String>,
}

impl TitleParams {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    pub fn insert(&mut self, k: impl Into<String>, v: impl Into<String>) {
        self.params.insert(k.into(), v.into());
    }
}

const MOSS_PREFIX: &str = "moss:";

/// Parse a title-attribute string. Returns None if the title does NOT start
/// with `moss:` (treating it as a native CommonMark title, not moss-params).
///
/// Malformed entries (missing `=`) are SKIPPED, not aborted — `moss:k1=v1 k2 k3=v3`
/// yields {k1=v1, k3=v3}. This preserves forward compatibility: a future
/// param the parser doesn't understand doesn't break sibling params.
pub fn parse_title(title: &str) -> Option<TitleParams> {
    let rest = title.strip_prefix(MOSS_PREFIX)?;
    let mut out = TitleParams::default();
    let mut chars = rest.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }

        // Read key
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }

        // No `=` after key → malformed entry. Skip to next whitespace and continue
        // (don't abort — preserve sibling params).
        if chars.peek() != Some(&'=') {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
            }
            continue;
        }
        chars.next(); // consume '='

        // Read value (bare or quoted)
        let value = if chars.peek() == Some(&'"') {
            chars.next(); // consume opening quote
            let mut v = String::new();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(esc) = chars.next() {
                        v.push(esc);
                    }
                } else if c == '"' {
                    break;
                } else {
                    v.push(c);
                }
            }
            v
        } else {
            let mut v = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                v.push(c);
                chars.next();
            }
            v
        };

        // Only insert if key is non-empty (defensive — leading `=` would be malformed).
        if !key.is_empty() {
            out.params.insert(key, value);
        }
    }

    Some(out)
}

/// Emit a title-attribute string. Always includes the `moss:` prefix.
/// Empty params produce `"moss:"` (still recognized as moss-extension marker
/// — useful for "this is moss but no params" sentinel).
pub fn emit_title(params: &TitleParams) -> String {
    let mut out = String::from(MOSS_PREFIX);
    let mut first = true;
    for (k, v) in &params.params {
        if !first {
            out.push(' ');
        } else {
            first = false;
        }
        out.push_str(k);
        out.push('=');
        if v.contains(|c: char| c.is_whitespace() || c == '"') {
            out.push('"');
            for c in v.chars() {
                if c == '"' || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        } else {
            out.push_str(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_no_moss_prefix_returns_none() {
        assert_eq!(parse_title(""), None);
        assert_eq!(parse_title("Real title text"), None);
        assert_eq!(parse_title("moss"), None); // must have colon
    }

    #[test]
    fn parse_skips_malformed_entries_preserves_others() {
        // Forward-compat: a malformed `k2` (no `=`) does NOT abort parsing —
        // we skip it and continue. k1 and k3 are preserved.
        let p = parse_title("moss:k1=v1 k2 k3=v3").unwrap();
        assert_eq!(p.get("k1"), Some("v1"));
        assert_eq!(p.get("k2"), None);
        assert_eq!(p.get("k3"), Some("v3"));
    }

    #[test]
    fn parse_handles_empty_value() {
        let p = parse_title("moss:width=").unwrap();
        assert_eq!(p.get("width"), Some(""));
    }

    #[test]
    fn parse_empty_params() {
        let p = parse_title("moss:").unwrap();
        assert!(p.is_empty());
    }

    #[test]
    fn parse_single_param() {
        let p = parse_title("moss:width=wide").unwrap();
        assert_eq!(p.get("width"), Some("wide"));
    }

    #[test]
    fn parse_multiple_params() {
        let p = parse_title("moss:width=wide align=left kind=video").unwrap();
        assert_eq!(p.get("width"), Some("wide"));
        assert_eq!(p.get("align"), Some("left"));
        assert_eq!(p.get("kind"), Some("video"));
    }

    #[test]
    fn parse_quoted_value() {
        let p = parse_title(r#"moss:caption="A nice photo""#).unwrap();
        assert_eq!(p.get("caption"), Some("A nice photo"));
    }

    #[test]
    fn parse_quoted_value_with_escape() {
        let p = parse_title(r#"moss:caption="He said \"hi\"""#).unwrap();
        assert_eq!(p.get("caption"), Some(r#"He said "hi""#));
    }

    #[test]
    fn emit_round_trip() {
        let mut p = TitleParams::default();
        p.insert("width", "wide");
        p.insert("align", "left");
        let s = emit_title(&p);
        // BTreeMap → alphabetical order
        assert_eq!(s, "moss:align=left width=wide");
        assert_eq!(parse_title(&s), Some(p));
    }

    #[test]
    fn emit_quotes_when_value_has_whitespace() {
        let mut p = TitleParams::default();
        p.insert("caption", "A nice photo");
        let s = emit_title(&p);
        assert_eq!(s, r#"moss:caption="A nice photo""#);
    }

    #[test]
    fn round_trip_preserves_unicode() {
        let mut p = TitleParams::default();
        p.insert("caption", "梁鸿: 中国一个村庄的当代史");
        let s = emit_title(&p);
        assert_eq!(parse_title(&s).unwrap(), p);
    }
}
