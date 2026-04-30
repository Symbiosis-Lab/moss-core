//! Typed shortcode AST nodes.
//!
//! Each shortcode is a closed enum variant with fully-typed arguments. The
//! variants are added during Phase B of the typed-AST migration; in Phase A
//! the enum is empty so the AST module compiles end-to-end.
//!
//! Migration order (Phase B): Subscribe, Buttons, Gallery, Hero, Grid.

use serde::{Deserialize, Serialize};

/// A typed shortcode block.
///
/// Phase A: empty stub. Variants land per-shortcode in Phase B (one
/// variant per migration commit). The empty enum is intentionally an
/// uninhabited type — `match sc {}` is exhaustive — so no consumer can
/// ship a half-migrated state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shortcode {}

/// Identifier for a shortcode kind, used for AST queries (e.g.
/// `has_shortcode(&doc, ShortcodeKind::Subscribe)` to gate feature
/// detection without scanning source files).
///
/// Kept as a separate enum rather than `std::mem::discriminant(&Shortcode)`
/// so callers can match on it without owning a `Shortcode` value, and so
/// the kind set is stable across feature additions.
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
    ///
    /// Phase A: unreachable since the enum is empty. Per-variant arms
    /// land in Phase B.
    pub fn kind(&self) -> ShortcodeKind {
        match *self {}
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
}
