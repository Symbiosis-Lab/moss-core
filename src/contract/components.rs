//! moss component contract — Source 2 of the federated contract.
//!
//! Single source of truth for every `moss-*` class moss currently emits.
//! Each entry declares: the class name, its kind (container/instance/standalone/chrome),
//! accepted `data-*` attributes with value spaces, example HTML, example markdown.
//!
//! ## Adding a new emitter class
//!
//! 1. Emit the class from your renderer module (`build/markdown/*`, `build/components/*`).
//! 2. Add a `ComponentEntry` to [`COMPONENTS`] here.
//! 3. Run `cargo test --test components_sync_test` from src-tauri/ — the
//!    scanner test will fail if you forget.
//! 4. Run `cargo run --bin generate-contract-docs --features dev-tools` to
//!    refresh `docs/contract/reference.md`.
//!
//! ## Why a const table, not a derive macro?
//!
//! Mirrors the BUILTIN_FIELDS precedent in `schema_fields.rs`. The synchronization
//! is enforced by a sync test (`emitter_classes_match_components_table`) that
//! scans emitter Rust source for `class="moss-..."` literals. This is a
//! best-effort scanner (won't catch classes assembled via `format!()`), not a
//! type-checked guarantee like BUILTIN_FIELDS' compile-time mirror. The
//! limitation is documented in the spec § Source 2.

/// Status of a component entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// In active use; theme authors can rely on it.
    Confirmed,
    /// Emerging convention; may evolve.
    Emerging,
    /// Scheduled for removal; theme authors should migrate.
    Retired,
}

/// A declared `data-*` attribute on a component.
pub struct DataAttr {
    /// Attribute name including `data-` prefix (e.g. `"data-layout"`).
    pub name: &'static str,
    /// Allowed values (e.g. `&["grid", "list", "minimal"]`). Empty means free-form.
    pub values: &'static [&'static str],
    /// Default value (first in `values`, or `""` for free-form).
    pub default: &'static str,
    /// Short description shown in reference.md.
    pub description: &'static str,
}

/// A single component contract entry.
pub struct ComponentEntry {
    /// Class name without leading `.` (e.g. `"moss-cards"`).
    pub class: &'static str,
    /// Container / Instance / Standalone / Chrome.
    pub kind: &'static str,
    /// For Instance kinds, the parent container's class (or `""`).
    pub parent: &'static str,
    /// Declared `data-*` attributes on the element with this class.
    pub data_attrs: &'static [DataAttr],
    /// Example HTML snippet showing the class in context. Multi-line allowed.
    pub example_html: &'static str,
    /// Example markdown that produces this HTML. Empty for HTML-only chrome.
    pub example_markdown: &'static str,
    /// Status: confirmed / emerging / retired.
    pub status: Status,
    /// Contract version this entry was introduced in.
    pub since: &'static str,
    /// Optional human-readable description.
    pub description: &'static str,
}

/// The full contract surface — every `moss-*` class moss currently emits.
///
/// Phase 0b seeds this with the CURRENT emitted vocabulary (not the
/// v1-collapsed shape). Phase 1c rewrites to the collapsed form.
pub const COMPONENTS: &[ComponentEntry] = &[
    ComponentEntry {
        class: "moss-cards",
        kind: "container",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-layout",
                values: &["grid", "list", "minimal"],
                default: "grid",
                description: "Card layout density. Grid: 2-3 cols with covers. List: single column with side covers. Minimal: text-only with year groupings.",
            },
            DataAttr {
                name: "data-density",
                values: &["default", "compact"],
                default: "default",
                description: "Vertical spacing density.",
            },
        ],
        example_html: r#"<div class="moss-cards" data-layout="grid">
  <a class="moss-card" href="...">...</a>
  <a class="moss-card" href="...">...</a>
</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Auto-generated listing of child pages. Today emitted as `.moss-card-grid` / `.moss-card-list` / `.moss-card-minimal`; collapsing into `.moss-cards[data-layout]` is Phase 1c.",
    },
    // Task 2 will add ~70 more entries here. Each subsequent entry follows
    // the same struct-literal shape.
];
