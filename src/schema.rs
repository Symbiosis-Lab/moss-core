//! Content model types driven by the schema.
//!
//! The schema defines frontmatter fields, types, and UI widget hints.
//! The built-in schema is generated from [`crate::schema_fields::BUILTIN_FIELDS`],
//! the single source of truth for all frontmatter fields moss recognizes.
//! Plugin-contributed schemas are still parsed from JSON via [`parse_schema()`].
//!
//! All types derive `Serialize` + `Deserialize` so they can cross the
//! Tauri command boundary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::schema_fields::BUILTIN_FIELDS;

/// The top-level content schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContentSchema {
    /// Generator that produced this schema (e.g. "moss").
    pub generator: String,
    /// Schema format version.
    pub version: String,
    /// Frontmatter field definitions.
    pub frontmatter: FrontmatterSchema,
    /// Optional shortcode definitions.
    pub shortcodes: Option<ShortcodeSchema>,
}

/// Frontmatter schema: a map of field name to definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct FrontmatterSchema {
    /// Field definitions keyed by field name.
    pub fields: HashMap<String, FieldDefinition>,
}

/// A single field definition in the frontmatter schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct FieldDefinition {
    /// The data type of the field.
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// Which UI widget to render for this field.
    /// `None` for sub-definitions (e.g. array item types) that have
    /// no direct UI representation.
    #[serde(default)]
    pub widget: Option<Widget>,
    /// Whether this field is required.
    #[serde(default)]
    pub required: bool,
    /// Default value for the field.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Format hint (e.g. "date" for YYYY-MM-DD).
    #[serde(default)]
    pub format: Option<String>,
    /// Allowed values for select/enum fields.
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    /// Item definition for array fields.
    #[serde(default)]
    #[cfg_attr(feature = "specta", specta(type = Option<serde_json::Value>))]
    pub items: Option<Box<FieldDefinition>>,
    /// Human-readable description of the field.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional human-readable label for the chip bar. When `None`, the frontend
    /// falls back to using the field key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Display priority for chip bar ordering. Lower values appear first.
    /// 0 means unset (defaults to end). Typical range: 10 (title) to 110 (cascade).
    #[serde(default)]
    pub priority: u8,
    /// Source of this field definition.
    /// `None` for builtin fields, `Some("review")` for plugin-contributed fields.
    /// Used by the frontend to group fields by source in the editor form.
    /// See docs/architecture/plugin-schema-contributions.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// UI group name for the add-property dropdown (e.g. "Common", "Children").
    /// `None` for plugin-contributed fields (shown in "Other" group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Supported field types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    String,
    Boolean,
    Integer,
    Number,
    Array,
    Object,
}

/// UI widget types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum Widget {
    TextInput,
    TextArea,
    DatePicker,
    NumberInput,
    Checkbox,
    Select,
    TagInput,
    FilePicker,
    CodeEditor,
}

/// Shortcode schema: delimiters and named definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ShortcodeSchema {
    /// (open_delimiter, close_delimiter) pair.
    pub delimiters: (String, String),
    /// Named shortcode definitions.
    pub definitions: HashMap<String, ShortcodeDefinition>,
}

/// A single shortcode definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ShortcodeDefinition {
    /// Display name.
    pub name: String,
    /// Whether the shortcode wraps content.
    #[serde(default)]
    pub has_content: bool,
    /// Parameter definitions.
    #[serde(default)]
    pub params: Vec<ShortcodeParam>,
}

/// A shortcode parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ShortcodeParam {
    /// Parameter name.
    pub name: String,
    /// Whether the parameter is required.
    #[serde(default)]
    pub required: bool,
    /// Default value.
    #[serde(default)]
    pub default: Option<String>,
}

/// Return the built-in content schema.
///
/// Builds the schema programmatically from [`BUILTIN_FIELDS`] — the const
/// table in `schema_fields.rs`. Fields with `skip_schema: true` are excluded.
/// This replaces the previous `include_str!("builtin-schema.json")` approach,
/// ensuring the schema and the `FrontMatter` struct can never drift apart.
pub fn builtin_schema() -> ContentSchema {
    let mut fields = HashMap::new();

    for bf in BUILTIN_FIELDS {
        if bf.skip_schema {
            continue;
        }

        let items = bf.items_type.as_ref().map(|it| {
            Box::new(FieldDefinition {
                field_type: it.clone(),
                widget: None,
                required: false,
                default: None,
                format: None,
                enum_values: None,
                items: None,
                description: None,
                label: None,
                priority: 0,
                source: None,
                group: None,
            })
        });

        let default = bf.default_json.map(|s| {
            serde_json::from_str(s)
                .unwrap_or_else(|e| panic!("invalid default_json for '{}': {}", bf.name, e))
        });

        let enum_values = bf
            .enum_values
            .map(|vals| vals.iter().map(|s| s.to_string()).collect());

        let fd = FieldDefinition {
            field_type: bf.field_type.clone(),
            widget: Some(bf.widget.clone()),
            required: bf.required,
            default,
            format: bf.format.map(|s| s.to_string()),
            enum_values,
            items,
            description: Some(bf.description.to_string()),
            label: bf.label.map(|s| s.to_string()),
            priority: bf.priority,
            source: None,
            group: if bf.group.is_empty() { None } else { Some(bf.group.to_string()) },
        };

        fields.insert(bf.name.to_string(), fd);
    }

    ContentSchema {
        generator: "moss".to_string(),
        version: "1.0".to_string(),
        frontmatter: FrontmatterSchema { fields },
        shortcodes: Some(ShortcodeSchema {
            delimiters: (":::".to_string(), ":::".to_string()),
            definitions: HashMap::new(),
        }),
    }
}

/// Parse a content schema from a JSON string.
pub fn parse_schema(json: &str) -> Result<ContentSchema, String> {
    serde_json::from_str(json).map_err(|e| format!("schema parse error: {}", e))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_schema_parses() {
        let schema = builtin_schema();
        assert_eq!(schema.generator, "moss");
        assert_eq!(schema.version, "1.0");
    }

    #[test]
    fn test_builtin_schema_has_required_title() {
        let schema = builtin_schema();
        let title = schema.frontmatter.fields.get("title").expect("title field");
        assert!(title.required);
        assert_eq!(title.field_type, FieldType::String);
        assert_eq!(title.widget, Some(Widget::TextInput));
    }

    #[test]
    fn test_builtin_schema_field_count() {
        let schema = builtin_schema();
        // 32 non-skip fields: 31 baseline + `sort` (2026-05-17 listing-sort
        // migration, Task 4). Baseline of 31 already exceeded the previous
        // count of 29 (test was stale).
        assert_eq!(schema.frontmatter.fields.len(), 32);
    }

    #[test]
    fn test_all_non_skip_fields_have_group() {
        let schema = builtin_schema();
        for (name, field) in &schema.frontmatter.fields {
            assert!(
                field.group.is_some(),
                "field '{}' is exposed in schema but has no group",
                name
            );
        }
    }

    #[test]
    fn test_builtin_schema_date_field() {
        let schema = builtin_schema();
        let date = schema.frontmatter.fields.get("date").expect("date field");
        assert_eq!(date.field_type, FieldType::String);
        assert_eq!(date.widget, Some(Widget::DatePicker));
        assert_eq!(date.format.as_deref(), Some("date"));
    }

    #[test]
    fn test_builtin_schema_children_boolean() {
        // D1: `children` is boolean — answers "render children or not?"
        let schema = builtin_schema();
        let children = schema.frontmatter.fields.get("children").expect("children field");
        assert_eq!(children.field_type, FieldType::Boolean, "children should be boolean");
        assert_eq!(children.widget, Some(Widget::Checkbox), "children should use checkbox widget");
        assert!(children.enum_values.is_none(), "children should not have enum_values");
    }

    #[test]
    fn test_builtin_schema_sidebar_field() {
        // sidebar: wikilink string for folder whose children appear in sidebar
        let schema = builtin_schema();
        let sidebar = schema.frontmatter.fields.get("sidebar").expect("sidebar field");
        assert_eq!(sidebar.field_type, FieldType::String, "sidebar should be string");
        assert_eq!(sidebar.widget, Some(Widget::TextInput), "sidebar should use text-input widget");
    }

    #[test]
    fn test_builtin_schema_also_in_array() {
        let schema = builtin_schema();
        let ai = schema.frontmatter.fields.get("also_in").expect("also_in field");
        assert_eq!(ai.field_type, FieldType::Array);
        assert_eq!(ai.widget, Some(Widget::TagInput));
        let items = ai.items.as_ref().expect("items");
        assert_eq!(items.field_type, FieldType::String);
    }

    #[test]
    fn test_builtin_schema_boolean_fields() {
        let schema = builtin_schema();
        for name in &["draft", "unlisted", "breadcrumb"] {
            let field = schema.frontmatter.fields.get(*name)
                .unwrap_or_else(|| panic!("{} field missing", name));
            assert_eq!(field.field_type, FieldType::Boolean, "{} should be boolean", name);
            assert_eq!(field.widget, Some(Widget::Checkbox), "{} should be checkbox", name);
        }
    }

    #[test]
    fn test_builtin_schema_integer_fields() {
        let schema = builtin_schema();
        for name in &["weight"] {
            let field = schema.frontmatter.fields.get(*name)
                .unwrap_or_else(|| panic!("{} field missing", name));
            assert_eq!(field.field_type, FieldType::Integer, "{} should be integer", name);
            assert_eq!(field.widget, Some(Widget::NumberInput), "{} should be number-input", name);
        }
    }

    #[test]
    fn test_builtin_schema_shortcodes() {
        let schema = builtin_schema();
        let sc = schema.shortcodes.as_ref().expect("shortcodes");
        assert_eq!(sc.delimiters, (":::".to_string(), ":::".to_string()));
    }

    #[test]
    fn test_parse_schema_invalid_json() {
        let result = parse_schema("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("schema parse error"));
    }

    #[test]
    fn test_parse_schema_missing_fields() {
        let json = r#"{"generator":"test","version":"1.0"}"#;
        let result = parse_schema(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_serialization() {
        let schema = builtin_schema();
        let json = serde_json::to_string(&schema).expect("serialize");
        let parsed = parse_schema(&json).expect("re-parse");
        assert_eq!(schema.generator, parsed.generator);
        assert_eq!(schema.frontmatter.fields.len(), parsed.frontmatter.fields.len());
    }

    #[test]
    fn test_field_type_serde() {
        // Verify the lowercase rename works
        let json = r#""string""#;
        let ft: FieldType = serde_json::from_str(json).expect("parse field type");
        assert_eq!(ft, FieldType::String);

        let json = r#""boolean""#;
        let ft: FieldType = serde_json::from_str(json).expect("parse boolean");
        assert_eq!(ft, FieldType::Boolean);
    }

    #[test]
    fn test_widget_serde() {
        // Verify the kebab-case rename works
        let json = r#""text-input""#;
        let w: Widget = serde_json::from_str(json).expect("parse widget");
        assert_eq!(w, Widget::TextInput);

        let json = r#""date-picker""#;
        let w: Widget = serde_json::from_str(json).expect("parse date-picker");
        assert_eq!(w, Widget::DatePicker);
    }

    #[test]
    fn test_builtin_fields_have_no_source() {
        let schema = builtin_schema();
        for (name, field) in &schema.frontmatter.fields {
            assert!(field.source.is_none(), "builtin field '{}' should have no source", name);
        }
    }

    #[test]
    fn test_field_definition_with_source_roundtrips() {
        let json = r#"{"type":"string","widget":"text-input","source":"review"}"#;
        let fd: FieldDefinition = serde_json::from_str(json).expect("parse");
        assert_eq!(fd.source, Some("review".to_string()));
        let serialized = serde_json::to_string(&fd).expect("serialize");
        assert!(serialized.contains(r#""source":"review""#));
    }

    #[test]
    fn test_field_definition_without_source_omits_it() {
        let json = r#"{"type":"string","widget":"text-input"}"#;
        let fd: FieldDefinition = serde_json::from_str(json).expect("parse");
        assert!(fd.source.is_none());
        let serialized = serde_json::to_string(&fd).expect("serialize");
        assert!(!serialized.contains("source"));
    }
}
