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

/// Classes moss emits without the `moss-` prefix, and may keep emitting.
///
/// This list is closed by intent: a NEW component gets a `moss-` prefix. An
/// entry here is a class the renderer already shipped under a bare name, where
/// renaming it would break themes in the wild for no gain. Adding to it is a
/// decision, not a formality — which is the whole reason this is a named
/// constant rather than another clause on a growing `||` chain.
///
/// `main-nav` set the precedent; the five masthead classes followed it when
/// they were declared in the contract, and this list was not extended with
/// them at the time. Nothing caught that: `develop` runs no CI, so the gap
/// stayed green until the release PR.
const LEGACY_UNPREFIXED: &[&str] = &[
    "main-nav",
    // Article masthead — the byline row a themer reaches for first.
    "date-line",
    "date",
    "site-name",
    "breadcrumb-segment",
    "nav-icons",
];

#[test]
fn every_component_has_a_class_name() {
    // The `callout`, `callout-title`, `callout-content`, and `callout-<type>`
    // variants are emitted alongside `moss-callout` for theme parity with
    // Obsidian-style callouts, so they are matched by shape rather than listed.
    let is_legacy_callout = |c: &str| c == "callout" || c.starts_with("callout-");
    for entry in COMPONENTS {
        assert!(
            entry.class.starts_with("moss-")
                || LEGACY_UNPREFIXED.contains(&entry.class)
                || is_legacy_callout(entry.class),
            "class '{}' must be moss-prefixed, or be added to LEGACY_UNPREFIXED \
             with a reason it cannot carry the prefix",
            entry.class
        );
    }
}

#[test]
fn the_legacy_unprefixed_list_does_not_outlive_its_entries() {
    // A stale exemption is how a list like this rots: a class gets renamed to
    // `moss-*`, its entry here stays, and the next unprefixed class to arrive
    // finds a hole already open for it. Every entry must still name a real
    // component.
    for legacy in LEGACY_UNPREFIXED {
        assert!(
            COMPONENTS.iter().any(|e| &e.class == legacy),
            "'{legacy}' is exempted from the moss- prefix but is no longer in \
             COMPONENTS — drop it from LEGACY_UNPREFIXED"
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

/// Drift gate (arch-review #776): `ShortcodeKind::all()` is a hand-maintained
/// array, the SSOT for "which shortcodes are authorable". This test makes a
/// new enum variant impossible to add silently: the exhaustive `match` below
/// fails to COMPILE until the new variant is handled, and the count/coverage
/// assertions then fail until `all()` lists it — so a new shortcode can't skip
/// the authorable flag, example_markdown enforcement, or the render round-trip.
#[test]
fn shortcode_kind_all_enumerates_every_variant() {
    use moss_core::ast::shortcode::ShortcodeKind;
    let all: Vec<ShortcodeKind> = ShortcodeKind::all().collect();
    // Compile-time exhaustiveness: adding a variant breaks this match.
    for k in &all {
        match k {
            ShortcodeKind::Subscribe
            | ShortcodeKind::Buttons
            | ShortcodeKind::Gallery
            | ShortcodeKind::Hero
            | ShortcodeKind::Grid
            | ShortcodeKind::Recent
            | ShortcodeKind::Apply => {}
        }
    }
    let expected = [
        ShortcodeKind::Subscribe,
        ShortcodeKind::Buttons,
        ShortcodeKind::Gallery,
        ShortcodeKind::Hero,
        ShortcodeKind::Grid,
        ShortcodeKind::Recent,
        ShortcodeKind::Apply,
    ];
    assert_eq!(all.len(), expected.len(), "ShortcodeKind::all() must list every variant");
    for e in expected {
        assert!(all.contains(&e), "ShortcodeKind::all() is missing {e:?}");
        assert!(
            COMPONENTS.iter().any(|c| c.class == e.root_class()),
            "authorable shortcode {:?} maps to {} which is not in COMPONENTS",
            e, e.root_class()
        );
    }
}
