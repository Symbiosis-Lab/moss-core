//! W3C Design Tokens loader.
//!
//! Reads the embedded `tokens.json` (W3C Design Tokens Community Group format)
//! and exposes it as ordered structs the codegen consumes.
//!
//! ## Invariants
//! - Tokens are loaded at compile time via `include_str!`. moss-core stays zero-I/O.
//! - Group order is taken from the top-level `$order` array in tokens.json
//!   (NOT JSON insertion order — serde_json doesn't preserve insertion order
//!   by default and moss doesn't enable the `preserve_order` feature).
//! - Within each group, entries are sorted alphabetically.

const TOKENS_JSON: &str = include_str!("tokens.json");

/// A single design token entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEntry {
    /// CSS variable name without leading `--` (e.g. `moss-color-accent`).
    pub name: String,
    /// CSS value as a string (e.g. `#2d5a2d`, `1.125rem`, `var(--moss-content-width)`).
    /// When `$value` is an object with `"light"` and `"dark"` keys, this holds the light value.
    pub value: String,
    /// Dark-mode CSS value. `None` when `$value` is a plain string (light-only token).
    /// `Some(...)` when `$value` is `{ "light": "...", "dark": "..." }`.
    pub dark_value: Option<String>,
    /// Optional W3C `$type` hint (color, dimension, fontFamily, number).
    pub type_hint: Option<String>,
    /// Optional human-readable description.
    pub description: Option<String>,
}

/// A group of tokens (e.g. `typography`, `color`, `layout`, `spacing`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGroup {
    /// Group name as it appears in `tokens.json` (e.g. `color`).
    pub name: String,
    /// Optional group-level description.
    pub description: Option<String>,
    /// Token entries, sorted alphabetically.
    pub entries: Vec<TokenEntry>,
}

/// The full tokens manifest.
#[derive(Debug, Clone)]
pub struct Tokens {
    /// Groups in declared order (from `$order`).
    pub groups: Vec<TokenGroup>,
}

/// Load the embedded `tokens.json` into structured form.
///
/// Group order is taken from the top-level `$order` array in tokens.json.
/// Entries within each group are alphabetical.
///
/// Returns an error if the JSON is malformed or `$order` is missing.
pub fn load_tokens() -> Result<Tokens, String> {
    parse_tokens(TOKENS_JSON)
}

/// Parse a tokens.json string. Exposed for testing error paths;
/// production callers use `load_tokens()`.
pub fn parse_tokens(input: &str) -> Result<Tokens, String> {
    let value: serde_json::Value = serde_json::from_str(input)
        .map_err(|e| format!("tokens.json parse error: {}", e))?;
    let top = value.as_object().ok_or("tokens.json must be a JSON object")?;

    // Read group ordering from the explicit `$order` array.
    let order: Vec<String> = top
        .get("$order")
        .and_then(|v| v.as_array())
        .ok_or("tokens.json missing top-level `$order` array")?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let mut groups = Vec::with_capacity(order.len());

    for group_name in &order {
        let group_value = top
            .get(group_name)
            .ok_or_else(|| format!("`$order` lists '{}' but group is missing", group_name))?;
        let group_obj = group_value
            .as_object()
            .ok_or_else(|| format!("group '{}' must be an object", group_name))?;

        let mut description = None;
        let mut entries = Vec::new();

        for (entry_key, entry_value) in group_obj {
            if entry_key == "$description" {
                description = entry_value.as_str().map(|s| s.to_string());
                continue;
            }
            if entry_key.starts_with('$') {
                continue;
            }
            let entry_obj = entry_value
                .as_object()
                .ok_or_else(|| format!("entry '{}/{}' must be an object", group_name, entry_key))?;

            let type_hint = entry_obj.get("$type").and_then(|v| v.as_str()).map(String::from);
            let raw_value = entry_obj
                .get("$value")
                .ok_or_else(|| format!("entry '{}/{}' missing $value", group_name, entry_key))?;
            let (entry_value_str, entry_dark_value) = match raw_value {
                serde_json::Value::String(s) => (s.clone(), None),
                serde_json::Value::Object(obj) => {
                    let light = obj
                        .get("light")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| format!("entry '{}/{}' $value object missing \"light\" key", group_name, entry_key))?
                        .to_string();
                    let dark = obj.get("dark").and_then(|v| v.as_str()).map(String::from);
                    (light, dark)
                }
                _ => return Err(format!(
                    "entry '{}/{}' $value must be a string or {{\"light\",\"dark\"}} object",
                    group_name, entry_key
                )),
            };
            let entry_description = entry_obj.get("$description").and_then(|v| v.as_str()).map(String::from);

            entries.push(TokenEntry {
                name: entry_key.clone(),
                value: entry_value_str,
                dark_value: entry_dark_value,
                type_hint,
                description: entry_description,
            });
        }

        // Alphabetical within each group.
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        groups.push(TokenGroup {
            name: group_name.clone(),
            description,
            entries,
        });
    }

    Ok(Tokens { groups })
}

/// Format the loaded tokens as the CSS `:root` block per the v1 formatter
/// decisions (see spec § Open Question 3):
/// - Property order: group-then-alphabetical (groups in source order).
/// - Color casing: lowercase hex.
/// - Unit normalization: pass-through (tokens.json owns canonical units).
/// - Comments: blank line + group-name comment between groups.
/// - Indentation: 2 spaces.
/// - Trailing semicolons: always.
pub fn format_root_block(tokens: &Tokens) -> String {
    let mut out = String::new();
    out.push_str(":root {\n");

    for (idx, group) in tokens.groups.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        // Group name is title-cased: "typography" → "Typography".
        let title = title_case(&group.name);
        out.push_str(&format!("  /* {} */\n", title));

        for entry in &group.entries {
            let value = normalize_value(&entry.value, entry.type_hint.as_deref());
            out.push_str(&format!("  --{}: {};\n", entry.name, value));
        }
    }

    out.push_str("}\n");
    out
}

/// Format the tokens whose `dark_value` is set as a CSS `:root[data-theme="dark"]` block.
///
/// Mirrors `format_root_block`'s style (group comments, 2-space indent, trailing
/// semicolons). Returns an empty `String` if no token has a dark value.
pub fn format_dark_root_block(tokens: &Tokens) -> String {
    // Check whether any dark values exist at all.
    let has_dark = tokens
        .groups
        .iter()
        .any(|g| g.entries.iter().any(|e| e.dark_value.is_some()));
    if !has_dark {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(":root[data-theme=\"dark\"] {\n");

    let mut first_group = true;
    for group in &tokens.groups {
        // Only include groups that have at least one dark token.
        let dark_entries: Vec<&TokenEntry> = group
            .entries
            .iter()
            .filter(|e| e.dark_value.is_some())
            .collect();
        if dark_entries.is_empty() {
            continue;
        }

        if !first_group {
            out.push('\n');
        }
        first_group = false;

        let title = title_case(&group.name);
        out.push_str(&format!("  /* {} */\n", title));

        for entry in dark_entries {
            let dark_val = entry.dark_value.as_deref().unwrap();
            let value = normalize_value(dark_val, entry.type_hint.as_deref());
            out.push_str(&format!("  --{}: {};\n", entry.name, value));
        }
    }

    out.push_str("}\n");
    out
}

/// Format the dark-value tokens as a system-preference fallback block.
///
/// Produces:
/// ```css
/// @media (prefers-color-scheme: dark) {
///   :root:not([data-theme]) {
///     --moss-color-bg: #1c1914;
///     ...
///   }
/// }
/// ```
///
/// Only applies when NO explicit `data-theme` is set. Once the user or a
/// script sets any `data-theme`, the explicit `:root[data-theme="dark"]` block
/// (from `format_dark_root_block`) takes over.
///
/// Returns an empty `String` if no token has a dark value.
pub fn format_dark_media_block(tokens: &Tokens) -> String {
    // Check whether any dark values exist at all.
    let has_dark = tokens
        .groups
        .iter()
        .any(|g| g.entries.iter().any(|e| e.dark_value.is_some()));
    if !has_dark {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("@media (prefers-color-scheme: dark) {\n");
    out.push_str(":root:not([data-theme]) {\n");

    let mut first_group = true;
    for group in &tokens.groups {
        let dark_entries: Vec<&TokenEntry> = group
            .entries
            .iter()
            .filter(|e| e.dark_value.is_some())
            .collect();
        if dark_entries.is_empty() {
            continue;
        }

        if !first_group {
            out.push('\n');
        }
        first_group = false;

        let title = title_case(&group.name);
        out.push_str(&format!("  /* {} */\n", title));

        for entry in dark_entries {
            let dark_val = entry.dark_value.as_deref().unwrap();
            let value = normalize_value(dark_val, entry.type_hint.as_deref());
            out.push_str(&format!("  --{}: {};\n", entry.name, value));
        }
    }

    out.push_str("}\n");
    out.push_str("}\n");
    out
}

/// Title-case the group name. "typography" → "Typography", "color" → "Color".
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

/// Normalize a token value per the v1 formatter rules.
fn normalize_value(value: &str, type_hint: Option<&str>) -> String {
    if matches!(type_hint, Some("color")) {
        return normalize_hex_color(value);
    }
    value.to_string()
}

/// Normalize a hex color to lowercase 6-digit form. Pass through any value
/// that isn't a recognized hex literal (e.g., `var()`, `rgb()`, named colors).
fn normalize_hex_color(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix('#') {
        if rest.chars().all(|c| c.is_ascii_hexdigit())
            && (rest.len() == 3 || rest.len() == 6 || rest.len() == 8)
        {
            let lower = rest.to_lowercase();
            // Expand 3-digit hex to 6-digit.
            if lower.len() == 3 {
                let r = &lower[0..1];
                let g = &lower[1..2];
                let b = &lower[2..3];
                return format!("#{r}{r}{g}{g}{b}{b}");
            }
            return format!("#{}", lower);
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_dark_root_block_emits_only_dark_tokens() {
        let json = r##"{ "$order": ["color"], "color": {
          "moss-color-bg": {"$type":"color","$value":{"light":"#faf8f5","dark":"#1c1914"}},
          "moss-color-accent": {"$type":"color","$value":"#2d5a2d"} } }"##;
        let t = parse_tokens(json).unwrap();
        let dark = format_dark_root_block(&t);
        assert!(dark.contains(":root[data-theme=\"dark\"]"));
        assert!(dark.contains("--moss-color-bg: #1c1914"));
        assert!(!dark.contains("--moss-color-accent")); // no dark value → not emitted
    }

    #[test]
    fn format_dark_root_block_returns_empty_when_no_dark_values() {
        let json = r##"{ "$order": ["color"], "color": {
          "moss-color-accent": {"$type":"color","$value":"#2d5a2d"},
          "moss-color-bg": {"$type":"color","$value":"#faf8f5"} } }"##;
        let t = parse_tokens(json).unwrap();
        assert_eq!(format_dark_root_block(&t), "");
    }

    #[test]
    fn parse_tokens_accepts_object_value_with_dark() {
        let json = r##"{ "$order": ["color"], "color": { "moss-color-bg": {
            "$type": "color",
            "$value": { "light": "#faf8f5", "dark": "#1c1914" },
            "$description": "Page background" } } }"##;
        let tokens = parse_tokens(json).expect("parses");
        let bg = tokens.groups.iter().flat_map(|g| &g.entries)
            .find(|t| t.name == "moss-color-bg").expect("bg token");
        assert_eq!(bg.value, "#faf8f5");
        assert_eq!(bg.dark_value.as_deref(), Some("#1c1914"));
    }

    #[test]
    fn parse_tokens_string_value_has_no_dark() {
        let json = r##"{ "$order": ["color"], "color": { "moss-color-accent": {
            "$type": "color", "$value": "#2d5a2d", "$description": "Accent" } } }"##;
        let t = parse_tokens(json).unwrap();
        let a = t.groups.iter().flat_map(|g| &g.entries).find(|t| t.name == "moss-color-accent").unwrap();
        assert_eq!(a.value, "#2d5a2d");
        assert_eq!(a.dark_value, None);
    }

    #[test]
    fn tokens_json_includes_internal_tokens() {
        let t = load_tokens().unwrap();
        let names: Vec<_> = t.groups.iter().flat_map(|g| &g.entries).map(|t| t.name.as_str()).collect();
        for n in [
            "moss-text-secondary",
            "moss-border-light",
            "moss-border-medium",
            "moss-accent-hover",
            "moss-code-background",
            "moss-code-border",
            "moss-code-accent-primary",
            "moss-code-accent-secondary",
            "moss-code-accent-tertiary",
            "moss-code-accent-quaternary",
            "moss-hl-keyword",
            "moss-hl-string",
            "moss-hl-comment",
            "moss-hl-number",
            "moss-hl-function",
            "moss-hl-type",
            "moss-hl-tag",
            "moss-hl-attr",
            "moss-hl-operator",
            "moss-hl-builtin",
            "moss-hl-meta",
            "moss-hl-deletion",
            "moss-hl-addition-bg",
            "moss-hl-deletion-bg",
            "moss-font-2xs",
            "moss-font-xs",
            "moss-font-sm",
            "moss-font-lg",
            "moss-font-xl",
            "moss-font-2xl",
            "moss-font-3xl",
            "moss-font-heading-weight",
        ] {
            assert!(names.contains(&n), "missing token: {n}");
        }
        assert!(names.len() >= 45, "expected >=45 tokens, got {}", names.len());
    }

    #[test]
    fn format_dark_media_block_wraps_dark_tokens_in_media_query() {
        let json = r##"{ "$order": ["color"], "color": {
          "moss-color-bg": {"$type":"color","$value":{"light":"#faf8f5","dark":"#1c1914"}},
          "moss-color-accent": {"$type":"color","$value":"#2d5a2d"} } }"##;
        let t = parse_tokens(json).unwrap();
        let media = format_dark_media_block(&t);
        assert!(media.contains("@media (prefers-color-scheme: dark)"), "must be wrapped in @media");
        assert!(media.contains(":root:not([data-theme])"), "must target :root:not([data-theme])");
        assert!(media.contains("--moss-color-bg: #1c1914"), "must contain dark value");
        assert!(!media.contains("--moss-color-accent"), "light-only token must not appear");
    }

    #[test]
    fn format_dark_media_block_returns_empty_when_no_dark_values() {
        let json = r##"{ "$order": ["color"], "color": {
          "moss-color-accent": {"$type":"color","$value":"#2d5a2d"} } }"##;
        let t = parse_tokens(json).unwrap();
        assert_eq!(format_dark_media_block(&t), "");
    }

    /// Task 1.2: assert moss-accent-hover is derived via color-mix in both modes.
    #[test]
    fn accent_hover_is_derived_from_accent_via_color_mix() {
        let t = load_tokens().unwrap();
        let entry = t
            .groups
            .iter()
            .flat_map(|g| &g.entries)
            .find(|e| e.name == "moss-accent-hover")
            .expect("moss-accent-hover token must exist");

        // Light value must use color-mix (derives from --moss-color-accent).
        assert!(
            entry.value.contains("color-mix"),
            "moss-accent-hover light value must contain 'color-mix', got: {:?}",
            entry.value
        );
        assert!(
            entry.value.contains("var(--moss-color-accent)"),
            "moss-accent-hover light value must reference var(--moss-color-accent), got: {:?}",
            entry.value
        );

        // Dark value must also be present and use color-mix (lightens accent on hover).
        let dark = entry
            .dark_value
            .as_deref()
            .expect("moss-accent-hover must have a dark value");
        assert!(
            dark.contains("color-mix"),
            "moss-accent-hover dark value must contain 'color-mix', got: {:?}",
            dark
        );
        assert!(
            dark.contains("var(--moss-color-accent)"),
            "moss-accent-hover dark value must reference var(--moss-color-accent), got: {:?}",
            dark
        );
    }
}
