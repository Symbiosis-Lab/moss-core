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
        description: "Page title",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "date",
        field_type: FieldType::String,
        widget: Widget::DatePicker,
        format: Some("date"),
        description: "Publication date (YYYY-MM-DD)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "weight",
        field_type: FieldType::Integer,
        widget: Widget::NumberInput,
        description: "Sort weight for ordering",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "url",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        description: "Custom URL path override",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "description",
        field_type: FieldType::String,
        widget: Widget::TextArea,
        description: "Page excerpt for SEO meta, og:description, and list previews",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "lang",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        description: "Language code (e.g. en, zh)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "translationKey",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        description: "Key to link translations of the same content",
        ..FIELD_DEFAULTS
    },

    // --- Visual presentation ---
    BuiltinField {
        name: "cover",
        field_type: FieldType::String,
        widget: Widget::FilePicker,
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
        description: "Whether to show in site navigation",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "draft",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Mark as draft (excluded from build)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "unlisted",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Exclude from listings but still accessible",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "breadcrumb",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Override site-wide breadcrumb setting for this page",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "footer",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Show as a link in the site footer",
        ..FIELD_DEFAULTS
    },

    // --- Organization ---
    BuiltinField {
        name: "tags",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        description: "Content tags for organization",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "also_in",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        description: "Cross-list this page in other sections",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "series",
        field_type: FieldType::Array,
        widget: Widget::TagInput,
        items_type: Some(FieldType::String),
        description: "Declares children as sequential series. Use true for weight-based ordering, or a list of wikilinks for explicit order.",
        ..FIELD_DEFAULTS
    },

    // --- Children rendering ---
    BuiltinField {
        name: "children",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        default_json: Some("true"),
        description: "Whether to render child pages below content (true = show, false = hide)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "sidebar",
        field_type: FieldType::String,
        widget: Widget::TextInput,
        description: "Wikilink to folder whose children appear in sidebar (e.g. [[News]])",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_style",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["list", "summary"]),
        default_json: Some("\"list\""),
        description: "How child pages are rendered",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_group",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["year", "none"]),
        description: "How children are grouped: year (default for list) or none (default for card)",
        ..FIELD_DEFAULTS
    },
    BuiltinField {
        name: "children_depth",
        field_type: FieldType::String,
        widget: Widget::Select,
        enum_values: Some(&["direct", "all"]),
        default_json: Some("\"direct\""),
        description: "Whether to include only immediate children or all descendants",
        ..FIELD_DEFAULTS
    },

    // --- Cascade ---
    BuiltinField {
        name: "cascade",
        field_type: FieldType::Object,
        widget: Widget::CodeEditor,
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

    // --- Plugin integration (skip_schema) ---
    BuiltinField {
        name: "comments",
        field_type: FieldType::Boolean,
        widget: Widget::Checkbox,
        description: "Per-page comment opt-in/opt-out (consumed by comments plugin)",
        skip_schema: true, // will move to comments plugin contributed schema
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
