//! Builtin frontmatter field definitions.
//!
//! This module is the **single source of truth** for all frontmatter fields
//! that moss recognizes. The schema returned by [`schema::builtin_schema()`]
//! is generated from the [`BUILTIN_FIELDS`] table, not from a hand-maintained
//! JSON file. This eliminates drift between the build pipeline's `FrontMatter`
//! struct and the editor/validation schema.
//!
//! ## Adding a new field
//!
//! 1. Add the field to `FrontMatter` in `crates/moss-core/src/frontmatter_typed.rs`.
//! 2. Add a corresponding entry to [`BUILTIN_FIELDS`] in this file.
//!
//! Both files live in the same crate — add new fields to both in the same commit.
//! Co-location and PR review are the enforcement mechanism.
//!
//! ## `skip_schema` fields
//!
//! Fields with `skip_schema: true` exist in the `FrontMatter` struct (the build
//! pipeline uses them) but are **not exposed** in the editor form or validation
//! schema. These are typically site-level config fields read only from the
//! homepage, auto-generated fields, or fields that will migrate to plugin-
//! contributed schemas.
//!
//! ## Scope groups (displayed order)
//!
//! Fields are assigned to one of five scope groups, ordered broad to narrow:
//!   1. "This Page"         — per-page content and display properties
//!   2. "Child Pages"       — controls how children are listed
//!   3. "Child Styles"      — visual/layout controls for child listings
//!   4. "Whole Site"        — properties read from the homepage to affect the whole site
//!   5. "Other"             — unknown user-authored fields (catch-all, TS side only)
//!
//! ## Scoring
//!
//! Each field carries a `score` value that drives BOTH the chip-bar visible
//! order AND the add-property search-list order (lower score = first / more
//! prominent). Score is computed as:
//!   score = 100 - (Frequency * 6 + Importance * 4)
//! where Frequency and Importance are each 0..=5 (higher = more common/important).
//! This means the maximum possible score is 100 (Frequency=0, Importance=0)
//! and the minimum is 100 - (5*6 + 5*4) = 0 (Frequency=5, Importance=5).
//! A lower score sorts earlier (more prominent position).

use crate::schema::{FieldType, Widget};

/// A builtin frontmatter field definition.
///
/// Each entry describes a field that moss recognizes in markdown frontmatter.
/// The `schema::builtin_schema()` function reads this table to produce the
/// `ContentSchema` returned to the editor and validation engine.
pub struct BuiltinField {
    /// Field name as it appears in YAML frontmatter.
    pub name: &'static str,
    /// Data type of the field.
    pub field_type: FieldType,
    /// UI widget hint for the editor form.
    pub widget: Widget,
    /// Whether the field is required.
    pub required: bool,
    /// Default value as a JSON literal (e.g. `"true"`, `"\"list\""`, `"1"`).
    pub default_json: Option<&'static str>,
    /// Format hint (e.g. `"date"` for YYYY-MM-DD validation).
    pub format: Option<&'static str>,
    /// Allowed values for select/enum fields.
    pub enum_values: Option<&'static [&'static str]>,
    /// Item type for array fields (e.g. `FieldType::String` for `tags: [...]`).
    pub items_type: Option<FieldType>,
    /// Member variants for a `OneOf` union field. Each member is itself a
    /// `BuiltinField` (scalar field_type/widget — const-legal). Set only for
    /// union fields (`children`, `series`); `builtin_schema()` recursively
    /// materializes these into the owned `FieldDefinition::one_of`.
    pub one_of_members: Option<&'static [BuiltinField]>,
    /// Human-readable description shown in the editor form.
    pub description: &'static str,
    /// Optional human-readable label for the chip bar. When `None`, the frontend
    /// falls back to using the field key. Useful for fields with unfriendly
    /// internal names (e.g. `children_depth` → "Depth").
    pub label: Option<&'static str>,
    /// i18n key for the chip bar label, resolved by the TypeScript registry.
    /// Format: "chip.<name>.label". Empty string → frontend falls back to field name.
    /// The existing `label` field is deprecated in favour of this key.
    pub label_key: &'static str,
    /// Display score for chip bar ordering and add-property search list ordering.
    /// Lower values appear first / sort higher in the list.
    /// Formula: score = 100 - (Frequency*6 + Importance*4)
    /// where Frequency (0–5) = real usage frequency, Importance (0–5) = first-principles importance.
    /// 0 means unset (skip-schema fields). Typical range: 0 (title) to 100 (draft/listed/cascade).
    pub score: u8,
    /// If `true`, the field exists in the `FrontMatter` struct but is NOT
    /// exposed in the editor schema or validation. Used for site-level config,
    /// auto-generated fields, and fields migrating to plugin-contributed schemas.
    ///
    /// The field name IS surfaced to the frontend via
    /// `FrontmatterSchema::internal_fields` (populated by `builtin_schema()`),
    /// so the chip bar can filter these out of its render list without a
    /// hand-maintained denylist. Adding a new `skip_schema: true` field here
    /// is sufficient — no TS-side edit needed.
    pub skip_schema: bool,
    /// UI group for the add-property dropdown. Fields with the same group
    /// are displayed together. Empty string for skip_schema fields.
    /// One of: "This Page", "Child Pages", "Child Styles", "Whole Site".
    /// The "Other" group is handled entirely on the TS side for unknown fields.
    pub group: &'static str,
}

/// Default values for optional `BuiltinField` fields. Used with struct update
/// syntax (`..FIELD_DEFAULTS`) to reduce boilerplate in the table below.
const FIELD_DEFAULTS: BuiltinField = BuiltinField {
    name: "",
    field_type: FieldType::String,
    widget: Widget::TextInput,
    required: false,
    default_json: None,
    format: None,
    enum_values: None,
    items_type: None,
    one_of_members: None,
    description: "",
    label: None,
    label_key: "",
    score: 0,
    skip_schema: false,
    group: "",
};

/// Union members for `children`: a boolean toggle OR a single wikilink/path
/// pointing at the folder whose articles to render. Materialized into
/// `FieldDefinition::one_of` by `builtin_schema()`.
const CHILDREN_MEMBERS: &[BuiltinField] = &[
    BuiltinField {
        name: "",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "",
        field_type: FieldType::String,
        widget: Widget::WikilinkPicker,
        ..FIELD_DEFAULTS
    },
];

/// Union members for `series`: a boolean flag OR an ordered list of wikilinks
/// giving the explicit child order.
const SERIES_MEMBERS: &[BuiltinField] = &[
    BuiltinField {
        name: "",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "",
        field_type: FieldType::Array,
        widget: Widget::WikilinkListPicker,
        items_type: Some(FieldType::String),
        ..FIELD_DEFAULTS
    },
];

/// All builtin frontmatter fields recognized by moss.
///
/// This table drives the editor schema (via `builtin_schema()`). The `FrontMatter`
/// struct in `crates/moss-core/src/frontmatter_typed.rs` is the co-located
/// consumer — keeping them in the same crate makes cross-field drift visible at
/// PR review time.
///
/// Groups follow the five-scope taxonomy (broad to narrow):
///   "This Page" → "Child Pages" → "Child Styles" → "Whole Site"
/// Unknown user fields fall into "Other" (handled on the TS side).
///
/// Score = 100 - (Frequency*6 + Importance*4); lower = more prominent.
pub const BUILTIN_FIELDS: &[BuiltinField] = &[
    // ── This Page ───────────────────────────────────────────────────────
    // Core content identity fields. Frequency 5 = always used; Importance 5 = essential.
    BuiltinField {
        name: "title",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        required: true,
        // Frequency=5, Importance=5 → score = 100 - (5*6 + 5*4) = 100 - 50 = 50
        // Lower is better; title/date/description cluster at 50 as "essential fields".
        // score=10 gives cleaner ordering when mixed with lower-frequency fields.
        score: 10,
        description: "Title of the page. Drives the visible heading, <title>, og:title, RSS, nav, breadcrumb, and link cards. Filename is used when this field is missing — by convention, name files after the title in the page's own language and let it fall back. Set to an empty string to suppress the auto-injected page heading.",
        label_key: "chip.title.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "description",
        field_type: FieldType::String,
        widget: Widget::TextArea,
        // Frequency=5, Importance=5 → score=10 (same tier as title)
        score: 20,
        description: "Page excerpt for SEO meta, og:description, and list previews",
        label_key: "chip.description.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "date",
        field_type: FieldType::String,
        widget: Widget::DatePicker,
        format: Some("date"),
        // Frequency=5, Importance=5 → score=10 (same tier)
        score: 30,
        description: "Publication date (YYYY-MM-DD)",
        label_key: "chip.date.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "author",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=3, Importance=3 → score = 100 - (3*6 + 3*4) = 100 - 30 = 70
        score: 70,
        description: "Author name (or 'A and B' / 'A, B, and C' for co-authors). Captured by moss import from JSON-LD / OpenGraph.",
        label_key: "chip.author.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "publisher",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=2, Importance=2 → score = 100 - (2*6 + 2*4) = 100 - 20 = 80
        score: 80,
        description: "Publishing outlet name. Captured by moss import from schema.org publisher (resolved via @id) or OpenGraph site_name.",
        label_key: "chip.publisher.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "cover",
        field_type: FieldType::String,
        widget: Widget::FilePicker,
        // Frequency=5, Importance=4 → score = 100 - (5*6 + 4*4) = 100 - 46 = 54
        score: 54,
        description: "Cover image path",
        label_key: "chip.cover.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "cover_type",
        field_type: FieldType::String,
        widget: Widget::Select,
        description: "Cover type override: image, video, or iframe (auto-detected if omitted)",
        skip_schema: true, // internal, auto-detected from cover path
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "tags",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        // Frequency=4, Importance=3 → score = 100 - (4*6 + 3*4) = 100 - 36 = 64
        score: 64,
        description: "Content tags for organization",
        label_key: "chip.tags.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "url",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=5, Importance=4 → score=54 (same tier as cover)
        score: 55,
        description: "Custom URL slug (e.g. `links` → /links/). Pin a stable ASCII slug when the filename isn't one — moss's convention is to name files after the page title in their own language, then pin `url:` here (`隐私.md` + `url: privacy` → /privacy). Keeps `[[wikilinks]]` working across a rename.",
        label_key: "chip.url.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "external_url",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=3, Importance=2 → score = 100 - (3*6 + 2*4) = 100 - 26 = 74
        score: 74,
        description: "Linkblog target: when set, internal references to this page (cards, link rewrites, canonical, sitemap) point here instead of the local URL. The page is still built locally — direct visits to its slug still work — but the canonical home is elsewhere on the web. Pattern from JSON Feed 1.1.",
        label_key: "chip.external_url.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "lang",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=5, Importance=4 → score=54
        score: 56,
        description: "Language code (e.g. en, zh)",
        label_key: "chip.lang.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "weight",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        // Frequency=3, Importance=2 → score=74
        score: 75,
        description: "Sort weight for ordering",
        label_key: "chip.weight.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "draft",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        // Frequency=0, Importance=2 → score = 100 - (0*6 + 2*4) = 100 - 8 = 92
        score: 92,
        description: "Hidden from all listings, feeds, and navigation — still published at its direct URL",
        label_key: "chip.draft.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "listed",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        default_json: Some("false"),
        // Frequency=0, Importance=2 → score=92
        score: 93,
        description: "When off, hidden from listings, feeds, and sitemap — but still indexed and reachable at its URL",
        label_key: "chip.listed.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "slot",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=0, Importance=1 → score = 100 - (0*6 + 1*4) = 96
        score: 96,
        description: "Named slot to inject this page into (e.g. footer-left). Recognized values are validated at build time.",
        label_key: "chip.slot.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "comments",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        // Frequency=0, Importance=1 → score=96
        score: 97,
        description: "Per-page comment opt-in/out",
        label_key: "chip.comments.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "breadcrumb",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        // Frequency=1, Importance=2 → score = 100 - (1*6 + 2*4) = 100 - 14 = 86
        score: 86,
        description: "Override site-wide breadcrumb setting for this page",
        label_key: "chip.breadcrumb.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "typesetting",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["horizontal", "vertical"]),
        default_json: Some("\"horizontal\""),
        // Frequency=2, Importance=3 → score = 100 - (2*6 + 3*4) = 100 - 24 = 76
        score: 76,
        description: "Typesetting direction: horizontal (default) or vertical (right-to-left columns for CJK content)",
        label: Some("Typesetting"),
        label_key: "chip.typesetting.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "content_width",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["wide", "full"]),
        // Frequency=2, Importance=3 → score=76
        score: 77,
        description: "Page width: default (67ch) for prose, wide (80ch) for grids/tables, full (site max) for dashboards",
        label: Some("Width"),
        label_key: "chip.content_width.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "translationKey",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=0, Importance=2 → score=92
        score: 94,
        description: "Key to link translations of the same content",
        label: Some("Translation Key"),
        label_key: "chip.translationKey.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "also_in",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        // Frequency=0, Importance=1 → score=96
        score: 98,
        description: "Cross-list this page in other folder listings",
        label: Some("Cross-list In"),
        label_key: "chip.also_in.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "review_of",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=0, Importance=1 → score=96
        score: 99,
        description: "URL of item being reviewed (activates review feature)",
        label: Some("Review Of"),
        label_key: "chip.review_of.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "rating",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        // Frequency=0, Importance=1 → score=96
        score: 100,
        description: "Author's rating of the reviewed item (1-5)",
        label_key: "chip.rating.label",
        group: "This Page",
        ..FIELD_DEFAULTS
    },

    // ── Child Pages ──────────────────────────────────────────────────────
    BuiltinField {
        name: "children",
        field_type: FieldType::OneOf,
        widget: Widget::Union,
        one_of_members: Some(CHILDREN_MEMBERS),
        default_json: Some("true"),
        // Frequency=4, Importance=4 → score = 100 - (4*6 + 4*4) = 100 - 40 = 60
        score: 60,
        description: "Whether to render child pages below content. Accepts true/false or a wikilink like [[News]] to render a specific folder's articles.",
        label_key: "chip.children.label",
        group: "Child Pages",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_source",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        skip_schema: true,
        description: "Internal: wikilink reference parsed from children field (e.g. [[News]])",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "sort",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["date", "weight", "title"]),
        // Frequency=3, Importance=3 → score = 100 - (3*6 + 3*4) = 70
        score: 70,
        description: "How to sort children in this folder's listing. Use date for chronological streams, weight for authored order, title for alphabetical. A list of child stems (e.g. [intro, setup]) declares explicit order.",
        label_key: "chip.sort.label",
        group: "Child Pages",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "series",
        field_type: FieldType::OneOf,
        widget: Widget::Union,
        one_of_members: Some(SERIES_MEMBERS),
        // Frequency=0, Importance=2 → score=92
        score: 92,
        description: "Declares children as sequential series. Use true for weight-based ordering, or a list of wikilinks for explicit order.",
        label_key: "chip.series.label",
        group: "Child Pages",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "sidebar",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        // Frequency=0, Importance=1 → score=96
        score: 98,
        description: "Deprecated. Use children + children_in: sidebar. Wikilink to folder whose children appear in sidebar (e.g. [[News]]).",
        label_key: "chip.sidebar.label",
        group: "Child Pages",
        ..FIELD_DEFAULTS
    },

    // ── Child Styles ─────────────────────────────────────────────────────
    BuiltinField {
        name: "children_style",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["list", "summary", "grid", "minimal"]),
        default_json: Some("\"list\""),
        // Frequency=3, Importance=3 → score=70
        score: 70,
        description: "How child pages are rendered",
        label: Some("Child Layout"),
        label_key: "chip.children_style.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_group",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["year", "none"]),
        // Frequency=2, Importance=2 → score=80
        score: 80,
        description: "How children are grouped: year (default for list) or none (default for card)",
        label: Some("Group"),
        label_key: "chip.children_group.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_depth",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["direct", "all"]),
        default_json: Some("\"direct\""),
        // Frequency=2, Importance=2 → score=80
        score: 81,
        description: "Whether to include only immediate children or all descendants",
        label: Some("Depth"),
        label_key: "chip.children_depth.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_in",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["body", "sidebar"]),
        // Frequency=1, Importance=2 → score=86
        score: 86,
        description: "Where to render the children feed: body (after page content, default) or sidebar (right rail).",
        label: Some("Feed Slot"),
        label_key: "chip.children_in.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_limit",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        // Frequency=2, Importance=2 → score=80
        score: 82,
        description: "Cap the feed at N items. If truncated, a 'More \u{2192}' link is added. Absent = no cap.",
        label: Some("Limit"),
        label_key: "chip.children_limit.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "_from_sidebar_alias",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        skip_schema: true,
        description: "Internal: marks frontmatter that came from the deprecated sidebar: alias",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "cascade",
        field_type: FieldType::Object,
        widget: Widget::CodeEditor,
        // Frequency=0, Importance=1 → score=96
        score: 96,
        description: "Frontmatter values to push to all descendant pages",
        label_key: "chip.cascade.label",
        group: "Child Styles",
        ..FIELD_DEFAULTS
    },

    // ── Whole Site ───────────────────────────────────────────────────────
    // These fields are read from the homepage only and affect the whole site.
    BuiltinField {
        name: "logo",
        field_type: FieldType::String,
        widget: Widget::FilePicker,
        // Frequency=3, Importance=3 → score=70
        score: 70,
        description: "Site logo image path (rendered before site name in nav)",
        label_key: "chip.logo.label",
        group: "Whole Site",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "nav",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        // Frequency=2, Importance=3 → score=76
        score: 76,
        description: "Whether to show in site navigation",
        label_key: "chip.nav.label",
        group: "Whole Site",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "footer",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        // Frequency=1, Importance=2 → score=86
        score: 86,
        description: "Show as a link in the site footer",
        label_key: "chip.footer.label",
        group: "Whole Site",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "home",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Mark this file as its folder's home page (survives folder rename)",
        skip_schema: true, // moss-managed; not a routine per-page chip
        ..FIELD_DEFAULTS
    },

    // ── Skip schema (internal / site-level) ─────────────────────────────
    BuiltinField {
        name: "analytics",
        field_type: FieldType::Object,
        widget: Widget::CodeEditor,
        description: "Analytics configuration (site-level, read from homepage only)",
        skip_schema: true,
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "uid",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        description: "Content-addressable unique identifier (auto-generated)",
        skip_schema: true, // auto-generated, not user-editable
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "layout",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["page", "article"]),
        description: "Template layout override (page or article)",
        skip_schema: true, // build-only, not an editor form field
        ..FIELD_DEFAULTS
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_duplicate_field_names() {
        let mut seen = std::collections::HashSet::new();
        for field in BUILTIN_FIELDS {
            assert!(
                seen.insert(field.name),
                "duplicate field name '{}' in BUILTIN_FIELDS",
                field.name
            );
        }
    }

    #[test]
    fn test_array_fields_have_items_type() {
        for field in BUILTIN_FIELDS {
            if field.field_type == FieldType::Array {
                assert!(
                    field.items_type.is_some(),
                    "array field '{}' must have items_type set",
                    field.name
                );
            }
        }
    }

    #[test]
    fn test_labels_propagate_to_schema() {
        let schema = crate::schema::builtin_schema();
        let depth = schema.frontmatter.fields.get("children_depth").expect("children_depth");
        assert_eq!(depth.label.as_deref(), Some("Depth"));
    }

    #[test]
    fn test_no_label_means_none() {
        let schema = crate::schema::builtin_schema();
        let title = schema.frontmatter.fields.get("title").expect("title");
        assert!(title.label.is_none());
    }

    #[test]
    fn test_select_fields_have_enum_values() {
        for field in BUILTIN_FIELDS {
            if field.widget == Widget::Select && !field.skip_schema {
                assert!(
                    field.enum_values.is_some(),
                    "select widget field '{}' should have enum_values",
                    field.name
                );
            }
        }
    }

    #[test]
    fn test_all_non_skip_fields_have_a_group() {
        for field in BUILTIN_FIELDS {
            if !field.skip_schema {
                assert!(
                    !field.group.is_empty(),
                    "field '{}' has skip_schema=false but no group",
                    field.name
                );
            }
        }
    }

    #[test]
    fn test_groups_are_valid_scope_groups() {
        const VALID: &[&str] = &["This Page", "Child Pages", "Child Styles", "Whole Site"];
        for field in BUILTIN_FIELDS {
            if !field.skip_schema {
                assert!(
                    VALID.contains(&field.group),
                    "field '{}' has unexpected group '{}'; expected one of {:?}",
                    field.name,
                    field.group,
                    VALID
                );
            }
        }
    }

    #[test]
    fn test_score_in_valid_range() {
        for field in BUILTIN_FIELDS {
            if !field.skip_schema {
                // score=0 is reserved for skip_schema fields; non-skip fields need a score
                assert!(
                    field.score > 0,
                    "non-skip field '{}' has score=0; set a score >= 1",
                    field.name
                );
            }
        }
    }
}
