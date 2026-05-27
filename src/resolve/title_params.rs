//! In-process typed-params struct for moss embed synthesizers.
//!
//! Phase 3 PR4 (2026-05-27): the `moss:` title-attribute channel
//! retired. Stage 1 no longer encodes typed params into markdown image
//! titles; Stage 2 wikilink dispatch reads typed params directly from
//! pulldown-cmark's `LinkType::WikiLink` pothole text (parsed by
//! [`super::wikilink_dispatch::parse_pothole_params`]).
//!
//! What survives in this module:
//!
//! - [`TitleParams`] — the typed-param key/value bag. Every per-kind
//!   synthesizer (`render/iframe.rs`, `audio.rs`, `pdf.rs`, `video.rs`,
//!   `model.rs`, image via `render/image.rs`) takes `&TitleParams` as
//!   its first argument. The dispatcher constructs an instance from the
//!   wikilink pothole and hands it to the synth function.
//!
//! What retired:
//!
//! - `parse_title` (deserialized `moss:K=V K=V` from a markdown title) —
//!   no consumer remains. Wikilink potholes are parsed by
//!   [`super::wikilink_dispatch::parse_pothole_params`] instead.
//! - `emit_title` is being retired in PR3 alongside `format_img_tag`
//!   (the last remaining producer). Until then it lives below as a
//!   transitional helper; once PR3 lands it goes with `format_img_tag`.

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

/// Emit a title-attribute string. Always includes the `moss:` prefix.
/// Empty params produce `"moss:"` (still recognized as moss-extension marker
/// — useful for "this is moss but no params" sentinel).
///
/// **Pending retirement in PR3** alongside `crate::media::format_img_tag`
/// (the only remaining production caller). The output of this function is
/// emitted into the markdown stream but no consumer reads the `moss:` prefix
/// back — `parse_title` retired in PR4. Until PR3 lands, this stays as a
/// dead-write helper to keep `format_img_tag`'s test pinning intact.
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
    fn empty_params_emit_just_prefix() {
        let p = TitleParams::default();
        assert_eq!(emit_title(&p), "moss:");
    }

    #[test]
    fn emit_round_trip_shape() {
        let mut p = TitleParams::default();
        p.insert("width", "wide");
        p.insert("align", "left");
        let s = emit_title(&p);
        // BTreeMap → alphabetical order
        assert_eq!(s, "moss:align=left width=wide");
    }

    #[test]
    fn emit_quotes_when_value_has_whitespace() {
        let mut p = TitleParams::default();
        p.insert("caption", "A nice photo");
        let s = emit_title(&p);
        assert_eq!(s, r#"moss:caption="A nice photo""#);
    }
}
