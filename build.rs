//! Build script for moss-core: validates builtin-schema.json at compile time.
//!
//! The schema JSON is embedded via `include_str!` and parsed at runtime.
//! This build script catches invalid widget/type values during `cargo build`
//! instead of panicking at runtime.

use std::path::Path;

// Valid values derived from Widget and FieldType enums in schema.rs.
// When adding a new variant to either enum, add the kebab-case string here too.
const VALID_WIDGETS: &[&str] = &[
    "text-input", "text-area", "date-picker", "number-input",
    "checkbox", "select", "tag-input", "file-picker", "code-editor",
];
const VALID_TYPES: &[&str] = &[
    "string", "boolean", "integer", "number", "array", "object",
];

fn main() {
    let schema_path = Path::new("src/builtin-schema.json");
    println!("cargo:rerun-if-changed={}", schema_path.display());

    let json = std::fs::read_to_string(schema_path)
        .expect("failed to read builtin-schema.json");
    let value: serde_json::Value = serde_json::from_str(&json)
        .expect("builtin-schema.json is not valid JSON");

    let fields = value
        .get("frontmatter")
        .and_then(|fm| fm.get("fields"))
        .and_then(|f| f.as_object())
        .expect("builtin-schema.json missing frontmatter.fields object");

    for (name, def) in fields {
        if let Some(widget) = def.get("widget").and_then(|w| w.as_str()) {
            assert!(
                VALID_WIDGETS.contains(&widget),
                "builtin-schema.json: field '{}' has invalid widget '{}'. Valid widgets: {:?}",
                name, widget, VALID_WIDGETS,
            );
        }
        if let Some(field_type) = def.get("type").and_then(|t| t.as_str()) {
            assert!(
                VALID_TYPES.contains(&field_type),
                "builtin-schema.json: field '{}' has invalid type '{}'. Valid types: {:?}",
                name, field_type, VALID_TYPES,
            );
        }
    }
}
