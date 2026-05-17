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
    pub value: String,
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
            let entry_value_str = entry_obj
                .get("$value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("entry '{}/{}' missing $value", group_name, entry_key))?
                .to_string();
            let entry_description = entry_obj.get("$description").and_then(|v| v.as_str()).map(String::from);

            entries.push(TokenEntry {
                name: entry_key.clone(),
                value: entry_value_str,
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
