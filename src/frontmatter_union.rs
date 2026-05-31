//! Normalization for union-typed frontmatter fields (`children`, `series`).
//!
//! These fields accept more than one authored shape — `children` is a bool OR a
//! wikilink/path string; `series` is a bool OR an ordered list of wikilinks. The
//! schema models them as [`crate::schema::FieldType::OneOf`]; this module is the
//! SINGLE place that maps an authored value to its canonical resolved form.
//!
//! Both the build pipeline and the editor's save path call these functions, so
//! the compiler and the editor can never interpret a value differently. The
//! functions are pure (zero I/O) and operate on [`serde_yaml::Value`] so a caller
//! holding a typed struct value can feed it via `serde_yaml::to_value` and a
//! caller holding a raw map can feed the entry directly.
//!
//! The decision tables below are the contract — they are mirrored by the
//! `union_members_round_trip` sync test and by the per-row unit tests at the
//! bottom of this file. Changing a row means changing both.

use serde_yaml::Value;

/// Canonical resolved form of the `children` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildrenNorm {
    /// Whether to render a children feed at all.
    pub children: bool,
    /// The folder reference (wikilink `[[News]]` or resolved path
    /// `news/index.md`) when targeting a different folder; `None` for the
    /// page's own direct children.
    pub source: Option<String>,
}

/// Canonical resolved form of the `series` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesNorm {
    /// Whether the page participates in a sequential series.
    pub series: bool,
    /// Explicit child order (list of wikilinks) when authored as a list;
    /// `None` for the bool flag form (order falls back to `sort`/weight).
    pub order: Option<Vec<String>>,
}

/// Normalize an authored `children` value.
///
/// | Input value                         | Output                              |
/// |-------------------------------------|-------------------------------------|
/// | `Bool(true)`                        | `{children:true,  source:None}`     |
/// | `Bool(false)`                       | `{children:false, source:None}`     |
/// | `String("true")` / `String("false")`| parsed as the bool                  |
/// | `String("")`                        | `{children:false, source:None}`     |
/// | `String(s)` non-empty, non-bool     | `{children:true,  source:Some(s)}`  |
/// | absent / null / number / map / seq  | `{children:false, source:None}`     |
pub fn normalize_children(v: &Value) -> ChildrenNorm {
    match v {
        Value::Bool(b) => ChildrenNorm { children: *b, source: None },
        Value::String(s) => {
            let t = s.trim();
            match t {
                "true" => ChildrenNorm { children: true, source: None },
                "false" | "" => ChildrenNorm { children: false, source: None },
                _ => ChildrenNorm { children: true, source: Some(s.clone()) },
            }
        }
        _ => ChildrenNorm { children: false, source: None },
    }
}

/// Normalize an authored `series` value.
///
/// | Input value                    | Output                                  |
/// |--------------------------------|-----------------------------------------|
/// | `Bool(b)`                      | `{series:b, order:None}`                |
/// | `String("true")`/`"false"`     | parsed as the bool                      |
/// | `Sequence` of strings          | `{series:true, order:Some(strings)}`    |
/// | `Sequence` with any non-string | `{series:false, order:None}` (malformed)|
/// | empty `Sequence`               | `{series:true, order:None}`             |
/// | absent / other                 | `{series:false, order:None}`            |
pub fn normalize_series(v: &Value) -> SeriesNorm {
    match v {
        Value::Bool(b) => SeriesNorm { series: *b, order: None },
        Value::String(s) => match s.trim() {
            "true" => SeriesNorm { series: true, order: None },
            _ => SeriesNorm { series: false, order: None },
        },
        Value::Sequence(items) => {
            if items.is_empty() {
                return SeriesNorm { series: true, order: None };
            }
            let mut order = Vec::with_capacity(items.len());
            for it in items {
                match it {
                    Value::String(s) => order.push(s.clone()),
                    // Any non-string element ⇒ malformed; ignore the whole list.
                    _ => return SeriesNorm { series: false, order: None },
                }
            }
            SeriesNorm { series: true, order: Some(order) }
        }
        _ => SeriesNorm { series: false, order: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    // --- normalize_children: one assertion per decision-table row ---

    #[test]
    fn children_bool_true() {
        assert_eq!(
            normalize_children(&Value::Bool(true)),
            ChildrenNorm { children: true, source: None }
        );
    }

    #[test]
    fn children_bool_false() {
        assert_eq!(
            normalize_children(&Value::Bool(false)),
            ChildrenNorm { children: false, source: None }
        );
    }

    #[test]
    fn children_string_true_false() {
        assert_eq!(
            normalize_children(&s("true")),
            ChildrenNorm { children: true, source: None }
        );
        assert_eq!(
            normalize_children(&s("false")),
            ChildrenNorm { children: false, source: None }
        );
    }

    #[test]
    fn children_empty_string_is_off() {
        assert_eq!(
            normalize_children(&s("")),
            ChildrenNorm { children: false, source: None }
        );
    }

    #[test]
    fn children_wikilink() {
        assert_eq!(
            normalize_children(&s("[[News]]")),
            ChildrenNorm { children: true, source: Some("[[News]]".to_string()) }
        );
    }

    #[test]
    fn children_bare_path() {
        // B3: the compiler accepts resolved paths, not just wikilinks.
        assert_eq!(
            normalize_children(&s("news/index.md")),
            ChildrenNorm { children: true, source: Some("news/index.md".to_string()) }
        );
    }

    #[test]
    fn children_other_types_off() {
        assert_eq!(
            normalize_children(&Value::Null),
            ChildrenNorm { children: false, source: None }
        );
        assert_eq!(
            normalize_children(&Value::Number(3.into())),
            ChildrenNorm { children: false, source: None }
        );
    }

    // --- normalize_series: one assertion per decision-table row ---

    #[test]
    fn series_bool() {
        assert_eq!(
            normalize_series(&Value::Bool(true)),
            SeriesNorm { series: true, order: None }
        );
        assert_eq!(
            normalize_series(&Value::Bool(false)),
            SeriesNorm { series: false, order: None }
        );
    }

    #[test]
    fn series_list_of_strings() {
        let seq = Value::Sequence(vec![s("[[Ch 1]]"), s("[[Ch 2]]")]);
        assert_eq!(
            normalize_series(&seq),
            SeriesNorm {
                series: true,
                order: Some(vec!["[[Ch 1]]".to_string(), "[[Ch 2]]".to_string()])
            }
        );
    }

    #[test]
    fn series_malformed_list_off() {
        let seq = Value::Sequence(vec![s("[[Ch 1]]"), Value::Bool(true)]);
        assert_eq!(
            normalize_series(&seq),
            SeriesNorm { series: false, order: None }
        );
    }

    #[test]
    fn series_empty_list_is_flag() {
        assert_eq!(
            normalize_series(&Value::Sequence(vec![])),
            SeriesNorm { series: true, order: None }
        );
    }

    #[test]
    fn series_other_off() {
        assert_eq!(
            normalize_series(&Value::Null),
            SeriesNorm { series: false, order: None }
        );
    }

    // --- round-trip: a value re-expressed via serde_yaml::to_value (the build
    //     side's call shape) normalizes identically to the raw value (editor). ---

    #[test]
    fn children_roundtrip_via_to_value() {
        let raw = s("[[News]]");
        let reexpressed = serde_yaml::to_value("[[News]]").unwrap();
        assert_eq!(normalize_children(&raw), normalize_children(&reexpressed));
    }
}
