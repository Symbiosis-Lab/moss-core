//! Typed frontmatter structs for the build pipeline.
//!
//! Lives in moss-core so validation, the resolver, and src-tauri's pipeline
//! all share one definition. See ADR-018 for the boundary rule.

use serde::{Deserialize, Serialize};

/// Series declaration field: sequential mode (`series: true`) or explicit
/// wikilink order (`series: ["[[Ch 1]]", "[[Ch 2]]"]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(untagged)]
pub enum SeriesField {
    /// `series: true` — sequential mode, sort children by weight.
    Flag(bool),
    /// `series: ["[[Ch 1]]", "[[Ch 2]]"]` — explicit wikilink order.
    Ordered(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_field_flag_roundtrips() {
        let v: SeriesField = serde_yaml::from_str("true").unwrap();
        assert!(matches!(v, SeriesField::Flag(true)));
        let v: SeriesField = serde_yaml::from_str("false").unwrap();
        assert!(matches!(v, SeriesField::Flag(false)));
    }

    #[test]
    fn series_field_ordered_roundtrips() {
        let v: SeriesField = serde_yaml::from_str(r#"["[[Ch 1]]", "[[Ch 2]]"]"#).unwrap();
        assert!(matches!(v, SeriesField::Ordered(ref items) if items.len() == 2));
    }
}
