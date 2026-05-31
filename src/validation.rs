//! Schema-driven frontmatter validation.
//!
//! Validates parsed frontmatter against a [`ContentSchema`], producing
//! LSP-compatible [`Diagnostic`] messages. Checks include:
//!
//! - Required fields missing
//! - Type mismatches (e.g. string where boolean expected)
//! - Enum constraint violations
//! - Date format validation (YYYY-MM-DD)
//! - Unknown fields (reported as `Hint`)

use crate::schema::{ContentSchema, FieldType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Diagnostic severity levels (LSP-compatible integer values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Fatal error — the content cannot be published.
    Error = 1,
    /// Something likely wrong but not fatal.
    Warning = 2,
    /// Informational message.
    Info = 3,
    /// Suggestion or style hint.
    Hint = 4,
}

/// A validation diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Severity level.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Frontmatter field path (e.g. "title", "also_in[0]").
    pub path: Option<String>,
    /// Source line (1-based), if available.
    pub line: Option<usize>,
    /// Source column (1-based), if available.
    pub column: Option<usize>,
}

/// Validate parsed frontmatter against a content schema.
///
/// Returns a list of diagnostics. An empty list means the frontmatter is valid.
pub fn validate_frontmatter(
    fm: &HashMap<String, serde_yaml::Value>,
    schema: &ContentSchema,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Check each field defined in the schema.
    for (name, def) in &schema.frontmatter.fields {
        match fm.get(name) {
            None => {
                if def.required {
                    diags.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!("required field '{}' is missing", name),
                        path: Some(name.clone()),
                        line: None,
                        column: None,
                    });
                }
            }
            Some(value) => {
                // Type check.
                if !value_matches_def(value, def) {
                    diags.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "field '{}' has wrong type: expected {}, got {}",
                            name,
                            type_name(&def.field_type),
                            yaml_type_name(value),
                        ),
                        path: Some(name.clone()),
                        line: None,
                        column: None,
                    });
                }

                // Enum constraint check.
                if let Some(ref allowed) = def.enum_values {
                    if let Some(s) = value.as_str() {
                        if !allowed.contains(&s.to_string()) {
                            diags.push(Diagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "field '{}' has invalid value '{}'; allowed: {:?}",
                                    name, s, allowed
                                ),
                                path: Some(name.clone()),
                                line: None,
                                column: None,
                            });
                        }
                    }
                }

                // Date format validation for fields with format: "date".
                if def.format.as_deref() == Some("date") {
                    if let Some(s) = value.as_str() {
                        if !is_valid_date(s) {
                            diags.push(Diagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "field '{}' has invalid date format '{}'; expected YYYY-MM-DD",
                                    name, s
                                ),
                                path: Some(name.clone()),
                                line: None,
                                column: None,
                            });
                        }
                    }
                }

                // Array item type check.
                if def.field_type == FieldType::Array {
                    if let (Some(items_def), Some(seq)) = (&def.items, value.as_sequence()) {
                        for (i, item) in seq.iter().enumerate() {
                            if !value_matches_type(item, &items_def.field_type) {
                                diags.push(Diagnostic {
                                    severity: Severity::Error,
                                    message: format!(
                                        "field '{}[{}]' has wrong type: expected {}, got {}",
                                        name,
                                        i,
                                        type_name(&items_def.field_type),
                                        yaml_type_name(item),
                                    ),
                                    path: Some(format!("{}[{}]", name, i)),
                                    line: None,
                                    column: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Check for unknown fields (not in schema) — report as Hint.
    for key in fm.keys() {
        if !schema.frontmatter.fields.contains_key(key) {
            diags.push(Diagnostic {
                severity: Severity::Hint,
                message: format!("unknown field '{}' is not defined in the schema", key),
                path: Some(key.clone()),
                line: None,
                column: None,
            });
        }
    }

    diags
}

/// Check if a YAML value satisfies a field definition. For `OneOf` unions the
/// value must match at least one member; otherwise it's a plain type check.
fn value_matches_def(value: &serde_yaml::Value, def: &crate::schema::FieldDefinition) -> bool {
    if def.field_type == FieldType::OneOf {
        return match &def.one_of {
            Some(members) => members.iter().any(|m| value_matches_def(value, m)),
            // A OneOf with no declared members accepts nothing meaningful;
            // treat as permissive to avoid false positives on malformed schemas.
            None => true,
        };
    }
    value_matches_type(value, &def.field_type)
}

/// Check if a YAML value matches a scalar/array field type.
fn value_matches_type(value: &serde_yaml::Value, expected: &FieldType) -> bool {
    match expected {
        FieldType::String => value.is_string(),
        FieldType::Boolean => value.is_bool(),
        FieldType::Integer => {
            // Accept both i64 and u64.
            value.is_i64() || value.is_u64()
        }
        FieldType::Number => {
            // Accept integers and floats.
            value.is_number()
        }
        FieldType::Array => value.is_sequence(),
        FieldType::Object => value.is_mapping(),
        // OneOf is dispatched by value_matches_def before reaching here; a bare
        // OneOf with no members is permissive.
        FieldType::OneOf => true,
    }
}

/// Human-readable name for a field type.
fn type_name(ft: &FieldType) -> &'static str {
    match ft {
        FieldType::String => "string",
        FieldType::Boolean => "boolean",
        FieldType::Integer => "integer",
        FieldType::Number => "number",
        FieldType::Array => "array",
        FieldType::Object => "object",
        FieldType::OneOf => "one-of",
    }
}

/// Human-readable name for a YAML value's actual type.
fn yaml_type_name(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "boolean",
        serde_yaml::Value::Number(n) => {
            if n.is_f64() && !n.is_i64() && !n.is_u64() {
                "number"
            } else {
                "integer"
            }
        }
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "array",
        serde_yaml::Value::Mapping(_) => "object",
        serde_yaml::Value::Tagged(_) => "tagged",
    }
}

/// Validate a date string in YYYY-MM-DD format.
///
/// Requires exactly 10 characters: 4 digits, dash, 2 digits, dash, 2 digits.
fn is_valid_date(s: &str) -> bool {
    // Strict format: YYYY-MM-DD (exactly 10 chars)
    if s.len() != 10 {
        return false;
    }

    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    // Verify all digit positions are ASCII digits.
    for &i in &[0, 1, 2, 3, 5, 6, 8, 9] {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
    }

    // The byte-position checks above guarantee the three-segment shape and
    // ASCII-digit content, so the parses below cannot fail today. But "panics
    // only when the author was right" is the same shape that just bit us in
    // `date.rs` — refactor the byte checks above and these `.unwrap()`s become
    // a panic on user input. Use slice-pattern destructuring + `let-else`
    // instead so the compiler enforces the three-segment shape, and bail
    // cleanly via `Result::Err` rather than a panic if parsing ever fails.
    let parts: Vec<&str> = s.split('-').collect();
    let [year_str, month_str, day_str] = parts.as_slice() else {
        return false;
    };
    let Ok(year) = year_str.parse::<u32>() else {
        return false;
    };
    let Ok(month) = month_str.parse::<u32>() else {
        return false;
    };
    let Ok(day) = day_str.parse::<u32>() else {
        return false;
    };

    if year < 1 || month < 1 || month > 12 || day < 1 {
        return false;
    }

    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };

    day <= days_in_month
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::builtin_schema;

    fn make_fm(pairs: &[(&str, serde_yaml::Value)]) -> HashMap<String, serde_yaml::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn str_val(s: &str) -> serde_yaml::Value {
        serde_yaml::Value::String(s.to_string())
    }

    fn bool_val(b: bool) -> serde_yaml::Value {
        serde_yaml::Value::Bool(b)
    }

    fn int_val(n: i64) -> serde_yaml::Value {
        serde_yaml::Value::Number(serde_yaml::Number::from(n))
    }

    #[test]
    fn test_valid_frontmatter() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("My Page")),
            ("date", str_val("2024-01-15")),
            ("draft", bool_val(false)),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_missing_required_title() {
        let schema = builtin_schema();
        let fm = make_fm(&[("date", str_val("2024-01-15"))]);

        let diags = validate_frontmatter(&fm, &schema);
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("title"))
            .collect();
        assert_eq!(missing.len(), 1);
    }

    #[test]
    fn test_type_mismatch_string_for_boolean() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("draft", str_val("yes")), // should be boolean
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let type_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("wrong type"))
            .collect();
        assert_eq!(type_errs.len(), 1);
        assert!(type_errs[0].message.contains("draft"));
    }

    #[test]
    fn test_type_mismatch_boolean_for_string() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", bool_val(true)), // should be string
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let type_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("title"))
            .collect();
        assert_eq!(type_errs.len(), 1);
    }

    #[test]
    fn test_type_mismatch_string_for_integer() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("weight", str_val("heavy")), // should be integer
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let type_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("weight"))
            .collect();
        assert_eq!(type_errs.len(), 1);
    }

    #[test]
    fn test_enum_violation() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("children_style", str_val("table")), // not in ["list", "summary", "grid"]
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let enum_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("children_style"))
            .collect();
        assert_eq!(enum_errs.len(), 1);
        assert!(enum_errs[0].message.contains("table"));
    }

    #[test]
    fn test_enum_valid() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("children_style", str_val("list")),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let enum_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("children_style"))
            .collect();
        assert!(enum_errs.is_empty());
    }

    #[test]
    fn test_enum_summary_valid() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("children_style", str_val("summary")),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let enum_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("children_style"))
            .collect();
        assert!(enum_errs.is_empty());
    }

    #[test]
    fn test_enum_card_now_invalid() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("children_style", str_val("card")), // was valid, now invalid
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let enum_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("children_style"))
            .collect();
        assert_eq!(enum_errs.len(), 1);
        assert!(enum_errs[0].message.contains("card"));
    }

    #[test]
    fn test_invalid_date_format() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("date", str_val("01/15/2024")), // wrong format
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let date_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("date"))
            .collect();
        assert_eq!(date_warns.len(), 1);
    }

    #[test]
    fn test_valid_date_format() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("date", str_val("2024-02-29")), // leap year
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let date_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("date"))
            .collect();
        assert!(date_warns.is_empty());
    }

    #[test]
    fn test_invalid_leap_year() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("date", str_val("2023-02-29")), // not a leap year
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let date_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("date"))
            .collect();
        assert_eq!(date_warns.len(), 1);
    }

    #[test]
    fn test_unknown_fields_are_hints() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("custom_field", str_val("value")),
            ("another_unknown", int_val(42)),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let hints: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Hint)
            .collect();
        assert_eq!(hints.len(), 2);
    }

    #[test]
    fn test_array_item_type_validation() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            (
                "also_in",
                serde_yaml::Value::Sequence(vec![
                    str_val("section-a"),
                    serde_yaml::Value::Number(serde_yaml::Number::from(42)), // wrong type
                ]),
            ),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let arr_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("also_in[1]"))
            .collect();
        assert_eq!(arr_errs.len(), 1);
    }

    #[test]
    fn test_valid_integer_field() {
        let schema = builtin_schema();
        let fm = make_fm(&[
            ("title", str_val("Test")),
            ("weight", int_val(10)),
        ]);

        let diags = validate_frontmatter(&fm, &schema);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn test_empty_frontmatter_only_required_errors() {
        let schema = builtin_schema();
        let fm = HashMap::new();

        let diags = validate_frontmatter(&fm, &schema);
        // Only "title" is required in the builtin schema
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("title"));
    }

    // --- Date validation unit tests ---

    #[test]
    fn test_is_valid_date() {
        assert!(is_valid_date("2024-01-15"));
        assert!(is_valid_date("2024-02-29")); // leap year
        assert!(is_valid_date("2024-12-31"));
        assert!(is_valid_date("2000-02-29")); // century leap year

        assert!(!is_valid_date("2023-02-29")); // not leap year
        assert!(!is_valid_date("2024-13-01")); // month > 12
        assert!(!is_valid_date("2024-00-01")); // month 0
        assert!(!is_valid_date("2024-01-32")); // day > 31
        assert!(!is_valid_date("2024-04-31")); // April has 30 days
        assert!(!is_valid_date("not-a-date"));
        assert!(!is_valid_date("2024/01/15")); // wrong separator
        assert!(!is_valid_date("2024-1-5")); // this passes since parse() accepts it
        assert!(!is_valid_date("1900-02-29")); // not a leap year (divisible by 100 but not 400)
    }
}
