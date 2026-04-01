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
//! 1. Add the field to the `FrontMatter` struct in `src-tauri/src/compile/generator/markdown.rs`.
//! 2. Add a corresponding entry to [`BUILTIN_FIELDS`] in this file.
//! 3. Run `cargo test` — the sync test in `markdown.rs` will fail if you forget either side.
//!
//! ## `skip_schema` fields
//!
//! Fields with `skip_schema: true` exist in the `FrontMatter` struct (the build
//! pipeline uses them) but are **not exposed** in the editor form or validation
//! schema. These are typically site-level config fields read only from the
//! homepage, auto-generated fields, or fields that will migrate to plugin-
//! contributed schemas.

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
    /// Human-readable description shown in the editor form.
    pub description: &'static str,
    /// Optional human-readable label for the chip bar. When `None`, the frontend
    /// falls back to using the field key. Useful for fields with unfriendly
    /// internal names (e.g. `children_depth` → "Depth").
    pub label: Option<&'static str>,
    /// Display priority for chip bar ordering. Lower values appear first.
    /// 0 means unset (skip-schema fields). Typical range: 10 (title) to 110 (cascade).
    pub priority: u8,
    /// If `true`, the field exists in the `FrontMatter` struct but is NOT
    /// exposed in the editor schema or validation. Used for site-level config,
    /// auto-generated fields, and fields migrating to plugin-contributed schemas.
    pub skip_schema: bool,
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
    description: "",
    label: None,
    priority: 0,
    skip_schema: false,
};

/// All builtin frontmatter fields recognized by moss.
///
/// This table drives both the editor schema (via `builtin_schema()`) and the
/// drift detection test (which asserts every field here has a matching field
/// in the `FrontMatter` struct, and vice versa).
pub const BUILTIN_FIELDS: &[BuiltinField] = &[
    // --- Content metadata ---
    BuiltinField {
        name: "title",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        required: true,
        priority: 10,
        description: "Page title",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "date",
        field_type: FieldType::String,
        widget: Widget::DatePicker,
        format: Some("date"),
        priority: 30,
        description: "Publication date (YYYY-MM-DD)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "weight",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        priority: 70,
        description: "Sort weight for ordering",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "url",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        priority: 40,
        description: "Custom URL path override",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "description",
        field_type: FieldType::String,
        widget: Widget::TextArea,
        priority: 20,
        description: "Page excerpt for SEO meta, og:description, and list previews",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "lang",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        priority: 70,
        description: "Language code (e.g. en, zh)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "translationKey",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        priority: 70,
        description: "Key to link translations of the same content",
        label: Some("Translation Key"),
        ..FIELD_DEFAULTS
    },

    // --- Visual presentation ---
    BuiltinField {
        name: "cover",
        field_type: FieldType::String,
        widget: Widget::FilePicker,
        priority: 60,
        description: "Cover image path",
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

    // --- Visibility & navigation ---
    BuiltinField {
        name: "nav",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 80,
        description: "Whether to show in site navigation",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "draft",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 60,
        description: "Mark as draft (excluded from build)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "unlisted",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 80,
        description: "Exclude from listings but still accessible",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "breadcrumb",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 80,
        description: "Override site-wide breadcrumb setting for this page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "footer",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 80,
        description: "Show as a link in the site footer",
        ..FIELD_DEFAULTS
    },

    // --- Organization ---
    BuiltinField {
        name: "tags",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        priority: 50,
        description: "Content tags for organization",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "also_in",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        priority: 90,
        description: "Cross-list this page in other sections",
        label: Some("Also In"),
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "series",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        priority: 90,
        description: "Declares children as sequential series. Use true for weight-based ordering, or a list of wikilinks for explicit order.",
        ..FIELD_DEFAULTS
    },

    // --- Children rendering ---
    BuiltinField {
        name: "children",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        default_json: Some("true"),
        priority: 100,
        description: "Whether to render child pages below content (true = show, false = hide)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "sidebar",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        priority: 90,
        description: "Wikilink to folder whose children appear in sidebar (e.g. [[News]])",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_style",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["list", "summary"]),
        default_json: Some("\"list\""),
        priority: 100,
        description: "How child pages are rendered",
        label: Some("Style"),
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_group",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["year", "none"]),
        priority: 100,
        description: "How children are grouped: year (default for list) or none (default for card)",
        label: Some("Group"),
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_depth",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["direct", "all"]),
        default_json: Some("\"direct\""),
        priority: 100,
        description: "Whether to include only immediate children or all descendants",
        label: Some("Depth"),
        ..FIELD_DEFAULTS
    },

    // --- Cascade ---
    BuiltinField {
        name: "cascade",
        field_type: FieldType::Object,
        widget: Widget::CodeEditor,
        priority: 110,
        description: "Frontmatter values to push to all descendant pages",
        ..FIELD_DEFAULTS
    },

    // --- Site-level config (skip_schema) ---
    BuiltinField {
        name: "analytics",
        field_type: FieldType::Object,
        widget: Widget::CodeEditor,
        description: "Analytics configuration (site-level, read from homepage only)",
        skip_schema: true,
        ..FIELD_DEFAULTS
    },

    // --- Review metadata ---
    BuiltinField {
        name: "review_of",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        priority: 90,
        description: "URL of item being reviewed (activates review feature)",
        label: Some("Review Of"),
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "rating",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        priority: 90,
        description: "Author's rating of the reviewed item (1-5)",
        ..FIELD_DEFAULTS
    },

    // --- Per-page overrides ---
    BuiltinField {
        name: "comments",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        priority: 80,
        description: "Per-page comment opt-in/opt-out",
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
        name: "content_width",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["wide", "full"]),
        priority: 75,
        description: "Page width: default (67ch) for prose, wide (80ch) for grids/tables, full (site max) for dashboards",
        label: Some("Width"),
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
}
