//! Folder-listing sort: types and inference cascade.
//!
//! Pure Rust, zero I/O. Consumed by:
//!   - the build pipeline (scan pass, card renderer, series-nav)
//!   - the editor form (to show "inferred: date" next to undeclared sort:)
//!
//! See docs/plans/2026-05-17-listing-sort-and-embeds-design.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum SortAxis {
    Date,
    Weight,
    Title,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(untagged)]
pub enum SortField {
    Axis(SortAxis),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResolvedSort {
    pub axis: SortAxis,
    pub explicit_order: Option<Vec<String>>,
    /// Default value of `series:` chrome. True iff axis == Weight OR explicit_order is Some.
    pub series_default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_field_parses_axis_strings() {
        assert!(matches!(serde_yaml::from_str::<SortField>("date").unwrap(), SortField::Axis(SortAxis::Date)));
        assert!(matches!(serde_yaml::from_str::<SortField>("weight").unwrap(), SortField::Axis(SortAxis::Weight)));
        assert!(matches!(serde_yaml::from_str::<SortField>("title").unwrap(), SortField::Axis(SortAxis::Title)));
    }

    #[test]
    fn sort_field_parses_list() {
        let f: SortField = serde_yaml::from_str("[intro, setup, advanced]").unwrap();
        match f {
            SortField::List(items) => assert_eq!(items, vec!["intro", "setup", "advanced"]),
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn sort_field_rejects_unknown_axis() {
        assert!(serde_yaml::from_str::<SortField>("random").is_err());
    }
}
