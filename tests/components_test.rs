// `ComponentEntry` and `Status` are imported to verify they are part of the
// public API surface of the module, even though they're only used through
// field/value access on `COMPONENTS` entries below.
#[allow(unused_imports)]
use moss_core::contract::components::{ComponentEntry, Status, COMPONENTS};

#[test]
fn is_public_returns_false_for_retired_entries() {
    // moss-cards-grid is Retired — is_public() must return false.
    let retired = COMPONENTS
        .iter()
        .find(|e| e.class == "moss-cards-grid")
        .expect("moss-cards-grid must be in COMPONENTS");
    assert!(
        !retired.is_public(),
        "retired entry 'moss-cards-grid' must not be is_public()"
    );
}

#[test]
fn is_public_returns_true_for_confirmed_entries() {
    // moss-cards is Confirmed — is_public() must return true.
    let confirmed = COMPONENTS
        .iter()
        .find(|e| e.class == "moss-cards")
        .expect("moss-cards must be in COMPONENTS");
    assert!(
        confirmed.is_public(),
        "confirmed entry 'moss-cards' must be is_public()"
    );
}

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

#[test]
fn every_authorable_shortcode_has_nonempty_example_markdown() {
    use moss_core::ast::shortcode::ShortcodeKind;
    for kind in ShortcodeKind::all() {
        let cls = kind.root_class();
        let e = COMPONENTS
            .iter()
            .find(|e| e.class == cls)
            .unwrap_or_else(|| panic!("authorable class {cls} missing from COMPONENTS"));
        assert!(
            !e.example_markdown.is_empty(),
            "authorable shortcode {cls} needs example_markdown"
        );
    }
}

#[test]
fn authorable_example_markdown_renders_its_class() {
    use moss_core::ast::{parse, render_document, DefaultHooks, ResolvedUrl, Url, UrlKind};
    use moss_core::ast::shortcode::ShortcodeKind;
    use moss_core::ast::visit_urls_mut;
    for kind in ShortcodeKind::all() {
        let cls = kind.root_class();
        let md = COMPONENTS
            .iter()
            .find(|e| e.class == cls)
            .unwrap()
            .example_markdown;
        let mut doc = parse(md);
        // Resolve all Unresolved URLs to a trivial external href so
        // shortcodes that contain links or images (buttons, gallery) do not
        // hit the debug_assert for Unresolved URLs in DefaultHooks.
        visit_urls_mut(&mut doc, |url| {
            if matches!(url, Url::Unresolved(_)) {
                *url = Url::Resolved(ResolvedUrl {
                    href: "https://example.com/placeholder".to_string(),
                    kind: UrlKind::External,
                });
            }
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(
            html.contains(cls),
            "rendering {cls} example_markdown must emit class {cls}; got:\n{html}"
        );
    }
}
