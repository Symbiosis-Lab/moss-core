// `ComponentEntry` and `Status` are imported to verify they are part of the
// public API surface of the module, even though they're only used through
// field/value access on `COMPONENTS` entries below.
#[allow(unused_imports)]
use moss_core::contract::components::{ComponentEntry, Status, COMPONENTS};

#[test]
fn components_table_is_non_empty() {
    assert!(!COMPONENTS.is_empty(), "COMPONENTS must contain at least one entry");
}

#[test]
fn every_component_has_a_class_name() {
    // Legacy / Obsidian-compat classes that don't carry the `moss-` prefix.
    // The `callout`, `callout-title`, `callout-content`, and `callout-<type>`
    // variants are emitted alongside `moss-callout` for theme parity with
    // Obsidian-style callouts.
    let is_legacy_callout = |c: &str| c == "callout" || c.starts_with("callout-");
    for entry in COMPONENTS {
        assert!(
            entry.class.starts_with("moss-")
                || entry.class == "main-nav"
                || is_legacy_callout(entry.class),
            "class '{}' must be moss-prefixed (or be a legacy exception)",
            entry.class
        );
    }
}

#[test]
fn components_table_has_no_duplicate_classes() {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for entry in COMPONENTS {
        assert!(
            seen.insert(entry.class),
            "duplicate class in COMPONENTS: {}",
            entry.class
        );
    }
}

#[test]
fn moss_cards_entry_has_expected_shape() {
    let cards = COMPONENTS.iter().find(|e| e.class == "moss-cards")
        .expect("moss-cards must be in COMPONENTS");
    assert_eq!(cards.kind, "container");
    assert!(cards.data_attrs.iter().any(|a| a.name == "data-layout"));
}
