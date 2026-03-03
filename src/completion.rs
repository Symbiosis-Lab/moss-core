//! Completion types for editor autocomplete.
//!
//! Types only (no logic yet). These will be consumed by future
//! editor completion Tauri commands.

use serde::{Deserialize, Serialize};

/// A single completion suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Display label for the completion.
    pub label: String,
    /// What kind of completion this is.
    pub kind: CompletionKind,
    /// Optional detail text shown alongside the label.
    pub detail: Option<String>,
    /// Text to insert when the completion is accepted.
    /// If `None`, the `label` is inserted.
    pub insert_text: Option<String>,
}

/// The kind of completion item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    /// A frontmatter field name.
    Field,
    /// A field value (e.g. enum option).
    Value,
    /// A shortcode name.
    Shortcode,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_item_roundtrip() {
        let item = CompletionItem {
            label: "title".to_string(),
            kind: CompletionKind::Field,
            detail: Some("Page title (required)".to_string()),
            insert_text: Some("title: ".to_string()),
        };

        let json = serde_json::to_string(&item).expect("serialize");
        let parsed: CompletionItem = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.label, "title");
        assert_eq!(parsed.kind, CompletionKind::Field);
        assert_eq!(parsed.detail.as_deref(), Some("Page title (required)"));
        assert_eq!(parsed.insert_text.as_deref(), Some("title: "));
    }

    #[test]
    fn test_completion_kind_serde() {
        let json = r#""Field""#;
        let kind: CompletionKind = serde_json::from_str(json).expect("parse");
        assert_eq!(kind, CompletionKind::Field);

        let json = r#""Value""#;
        let kind: CompletionKind = serde_json::from_str(json).expect("parse");
        assert_eq!(kind, CompletionKind::Value);

        let json = r#""Shortcode""#;
        let kind: CompletionKind = serde_json::from_str(json).expect("parse");
        assert_eq!(kind, CompletionKind::Shortcode);
    }

    #[test]
    fn test_completion_item_minimal() {
        let item = CompletionItem {
            label: "draft".to_string(),
            kind: CompletionKind::Field,
            detail: None,
            insert_text: None,
        };

        let json = serde_json::to_string(&item).expect("serialize");
        assert!(json.contains("draft"));

        let parsed: CompletionItem = serde_json::from_str(&json).expect("deserialize");
        assert!(parsed.detail.is_none());
        assert!(parsed.insert_text.is_none());
    }
}
