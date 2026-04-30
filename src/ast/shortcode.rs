//! Typed shortcode AST nodes.
//!
//! Each shortcode is a closed enum variant with fully-typed arguments.
//! Variants land per-shortcode in Phase B (one variant per migration
//! commit) of the typed-AST migration.
//!
//! Migration order (Phase B): Subscribe, Buttons, Gallery, Hero, Grid.

use serde::{Deserialize, Serialize};

/// A typed shortcode block.
///
/// Variants:
/// - [`Shortcode::Subscribe`] — inline subscribe form (description + button)
///
/// Phase B migrations add one variant per commit. Empty enum was the
/// Phase A stub; Phase B Task 7 introduces the first real variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Shortcode {
    /// `:::subscribe` — inline newsletter signup form.
    ///
    /// Body is parsed as `key: value` lines; recognized keys are
    /// `description` and `button`. Unknown keys are ignored (matches
    /// the existing rewriter's behavior; structured diagnostics are
    /// out-of-scope for this migration — see plan §Out-of-scope).
    Subscribe(SubscribeShortcode),
}

/// Arguments for [`Shortcode::Subscribe`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeShortcode {
    /// Optional override for the form's descriptive text.
    pub description: Option<String>,
    /// Optional override for the submit button label.
    pub button: Option<String>,
}

/// Identifier for a shortcode kind, used for AST queries (e.g.
/// `has_shortcode(&doc, ShortcodeKind::Subscribe)` to gate feature
/// detection without scanning source files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcodeKind {
    Subscribe,
    Buttons,
    Gallery,
    Hero,
    Grid,
}

impl Shortcode {
    /// Return the [`ShortcodeKind`] of this shortcode.
    pub fn kind(&self) -> ShortcodeKind {
        match self {
            Shortcode::Subscribe(_) => ShortcodeKind::Subscribe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcode_kind_variants_are_distinct() {
        let kinds = [
            ShortcodeKind::Subscribe,
            ShortcodeKind::Buttons,
            ShortcodeKind::Gallery,
            ShortcodeKind::Hero,
            ShortcodeKind::Grid,
        ];
        let unique: std::collections::HashSet<_> = kinds.iter().collect();
        assert_eq!(unique.len(), kinds.len());
    }

    #[test]
    fn shortcode_kind_round_trips_through_serde() {
        for kind in [
            ShortcodeKind::Subscribe,
            ShortcodeKind::Buttons,
            ShortcodeKind::Gallery,
            ShortcodeKind::Hero,
            ShortcodeKind::Grid,
        ] {
            let s = serde_json::to_string(&kind).expect("serialize");
            let back: ShortcodeKind = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn subscribe_kind_method_returns_subscribe() {
        let sc = Shortcode::Subscribe(SubscribeShortcode::default());
        assert_eq!(sc.kind(), ShortcodeKind::Subscribe);
    }

    #[test]
    fn subscribe_with_description_and_button() {
        let sc = Shortcode::Subscribe(SubscribeShortcode {
            description: Some("Get updates".to_string()),
            button: Some("Subscribe".to_string()),
        });
        match &sc {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.description.as_deref(), Some("Get updates"));
                assert_eq!(args.button.as_deref(), Some("Subscribe"));
            }
        }
    }

    #[test]
    fn subscribe_default_has_none_description_and_button() {
        let args = SubscribeShortcode::default();
        assert!(args.description.is_none());
        assert!(args.button.is_none());
    }

    #[test]
    fn subscribe_round_trips_through_serde() {
        let sc = Shortcode::Subscribe(SubscribeShortcode {
            description: Some("d".to_string()),
            button: Some("b".to_string()),
        });
        let s = serde_json::to_string(&sc).expect("serialize");
        let back: Shortcode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(sc, back);
    }
}
