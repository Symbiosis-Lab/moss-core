//! Pure date primitives shared by the build pipeline and the editor file tree.
//!
//! Returns canonicalized `YYYY-MM-DD` strings. Lexicographic comparison of
//! these strings is correct chronological comparison.
//!
//! No `chrono` dep — moss-core's existing convention is `Option<String>`
//! for dates (see `frontmatter::FrontMatter::date`).

use serde_yaml::Value;
use std::collections::HashMap;

/// Read the frontmatter `date` field and normalize to `YYYY-MM-DD`.
///
/// Accepts:
/// - ISO date: `2025-11-15`
/// - ISO timestamp: `2025-11-15T10:30:00Z`, `2025-11-15T10:30:00`
/// - Slash form: `2025/11/15`
/// - YAML date type (when serde_yaml parses bare dates as `Value::String`)
///
/// Returns `None` when the field is absent or unparseable.
pub fn date_from_frontmatter(frontmatter: &HashMap<String, Value>) -> Option<String> {
    let raw = match frontmatter.get("date")? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    normalize_date(&raw)
}

/// Parse a `YYYY-MM-DD-…` prefix from a filename.
///
/// Returns the canonical `YYYY-MM-DD` string when the filename starts with
/// a valid date in this exact form, optionally followed by `-rest` and an
/// extension. Otherwise returns `None`.
///
/// Examples:
///   `2025-11-15-research-proposals.md` → `Some("2025-11-15")`
///   `2025-11-15.md`                    → `Some("2025-11-15")`
///   `news-2025-11-15.md`               → `None`
pub fn date_from_filename_prefix(filename: &str) -> Option<String> {
    if filename.len() < 10 {
        return None;
    }
    let head = &filename[..10];
    let after = &filename[10..];

    let bytes = head.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y = std::str::from_utf8(&bytes[0..4]).ok()?.parse::<u32>().ok()?;
    let m = std::str::from_utf8(&bytes[5..7]).ok()?.parse::<u32>().ok()?;
    let d = std::str::from_utf8(&bytes[8..10]).ok()?.parse::<u32>().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || y == 0 {
        return None;
    }

    if !after.is_empty() && !after.starts_with('-') && !after.starts_with('.') {
        return None;
    }

    Some(format!("{:04}-{:02}-{:02}", y, m, d))
}

/// Normalize a date-ish string to `YYYY-MM-DD`. Accepts:
///   - `YYYY-MM-DD`
///   - `YYYY-MM-DDTHH:MM:SS[Z]`
///   - `YYYY/MM/DD`
fn normalize_date(s: &str) -> Option<String> {
    let s = s.trim();
    let date_part = s.split('T').next().unwrap_or(s);
    let normalized: String = date_part.replace('/', "-");
    let mut parts = normalized.split('-');
    let y = parts.next()?.parse::<u32>().ok()?;
    let m = parts.next()?.parse::<u32>().ok()?;
    let d = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{:04}-{:02}-{:02}", y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;
    use std::collections::HashMap;

    fn fm(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn iso_date_passes_through() {
        assert_eq!(
            date_from_frontmatter(&fm(&[("date", "2025-11-15")])),
            Some("2025-11-15".to_string())
        );
    }

    #[test]
    fn iso_timestamp_truncated_to_date() {
        assert_eq!(
            date_from_frontmatter(&fm(&[("date", "2025-11-15T10:30:00Z")])),
            Some("2025-11-15".to_string())
        );
    }

    #[test]
    fn slash_form_normalized() {
        assert_eq!(
            date_from_frontmatter(&fm(&[("date", "2025/11/15")])),
            Some("2025-11-15".to_string())
        );
    }

    #[test]
    fn missing_field_returns_none() {
        assert_eq!(date_from_frontmatter(&fm(&[("title", "x")])), None);
    }

    #[test]
    fn malformed_returns_none() {
        assert_eq!(
            date_from_frontmatter(&fm(&[("date", "not a date")])),
            None
        );
    }

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(date_from_frontmatter(&fm(&[("date", "")])), None);
    }

    // --- date_from_filename_prefix ---

    #[test]
    fn filename_with_dated_slug() {
        assert_eq!(
            date_from_filename_prefix("2025-11-15-research-proposals.md"),
            Some("2025-11-15".to_string())
        );
    }

    #[test]
    fn filename_bare_date_md() {
        assert_eq!(
            date_from_filename_prefix("2025-11-15.md"),
            Some("2025-11-15".to_string())
        );
    }

    #[test]
    fn filename_no_prefix() {
        assert_eq!(date_from_filename_prefix("research-proposals.md"), None);
    }

    #[test]
    fn filename_date_in_middle_does_not_match() {
        assert_eq!(date_from_filename_prefix("news-2025-11-15.md"), None);
    }

    #[test]
    fn filename_invalid_date_returns_none() {
        assert_eq!(date_from_filename_prefix("2025-13-99-foo.md"), None);
    }

    #[test]
    fn filename_extensionless() {
        assert_eq!(
            date_from_filename_prefix("2025-11-15-foo"),
            Some("2025-11-15".to_string())
        );
    }
}
