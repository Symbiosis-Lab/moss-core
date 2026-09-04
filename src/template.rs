//! Pure logic for the page-templates feature: what kind a captured page is,
//! and how a template's frontmatter is reset when it's instantiated into a
//! new page.
//!
//! Template storage and file I/O live in src-tauri (`.moss/templates/`) —
//! this module never touches the filesystem. Frontmatter here is the same
//! untyped `HashMap<String, serde_yaml::Value>` `crate::frontmatter` reads
//! and writes, not the vault's typed `FrontMatter`/`BUILTIN_FIELDS` schema:
//! a template's fields are copied verbatim, including ones the schema
//! doesn't know about, so round-tripping through the typed struct (which has
//! no catch-all field) would silently drop them.

use std::collections::HashMap;
use serde_yaml::Value;

/// What a saved template produces when instantiated. Decided at capture time
/// from the tree's own home-file election (a folder's home file captures as
/// `Folder`), so the template store and the tree never disagree about it.
/// Serialized as `page` / `folder` in the store and across the Tauri seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum TemplateKind {
    /// A single new file.
    Page,
    /// A new folder with a seeded, self-named home file.
    Folder,
}

/// Reset a template's frontmatter for a new instance: `title` is cleared
/// (the untitled-first flow fills it in when the user commits the H1), `date`
/// is stamped to `now`, and `uid` is dropped — it is moss's per-page identity
/// (the join key for comments and redirects), so an instance must mint its
/// own rather than collide with the page it was captured from. Every other
/// field — layout, tags, cascade, and anything else the template carries — is
/// copied verbatim, since that's the point of templating them.
///
/// `now` is a caller-supplied `YYYY-MM-DD` string (moss-core has no `chrono`
/// dependency by convention — see `date.rs`) so this stays pure and
/// deterministic; the Tauri I/O boundary reads the clock, not this function.
pub fn instantiate_template_frontmatter(
    mut frontmatter: HashMap<String, Value>,
    now: &str,
) -> HashMap<String, Value> {
    frontmatter.remove("title");
    frontmatter.remove("uid");
    frontmatter.insert("date".to_string(), Value::String(now.to_string()));
    frontmatter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiate_clears_title_and_uid_and_stamps_date() {
        let mut fm = HashMap::new();
        fm.insert("title".to_string(), Value::String("Old Title".to_string()));
        fm.insert("uid".to_string(), Value::String("abc123".to_string()));
        fm.insert("date".to_string(), Value::String("2020-01-01".to_string()));

        let out = instantiate_template_frontmatter(fm, "2026-09-03");

        assert_eq!(out.get("title"), None);
        assert_eq!(out.get("uid"), None);
        assert_eq!(out.get("date"), Some(&Value::String("2026-09-03".to_string())));
    }

    #[test]
    fn instantiate_preserves_other_fields_verbatim() {
        let mut fm = HashMap::new();
        fm.insert("layout".to_string(), Value::String("article".to_string()));
        fm.insert("tags".to_string(), Value::Sequence(vec![Value::String("a".to_string())]));
        let mut cascade = serde_yaml::Mapping::new();
        cascade.insert(Value::String("nav".to_string()), Value::Bool(true));
        fm.insert("cascade".to_string(), Value::Mapping(cascade.clone()));

        let out = instantiate_template_frontmatter(fm, "2026-09-03");

        assert_eq!(out.get("layout"), Some(&Value::String("article".to_string())));
        assert_eq!(
            out.get("tags"),
            Some(&Value::Sequence(vec![Value::String("a".to_string())]))
        );
        assert_eq!(out.get("cascade"), Some(&Value::Mapping(cascade)));
    }

    #[test]
    fn instantiate_stamps_date_even_when_absent() {
        let fm = HashMap::new();
        let out = instantiate_template_frontmatter(fm, "2026-09-03");
        assert_eq!(out.get("date"), Some(&Value::String("2026-09-03".to_string())));
    }
}
