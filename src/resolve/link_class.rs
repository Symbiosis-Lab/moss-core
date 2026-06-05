//! Pure link classification against an injected URL index.
//! The editor implements `UrlIndex` over the inverted ArticleMap; the build
//! may implement it over page_map for the parity test. moss-core does NO I/O.

/// Classification of one link target against the deployed URL space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkClass {
    /// Matches a canonical deployed URL exactly (case-sensitive).
    Resolved { url: String },
    /// A page exists, but this link won't hit its canonical URL (case/slug).
    Mismatch { canonical: String },
    /// Internal reference/absolute path with no deployed page (best-effort).
    Broken,
    /// http(s)/protocol-relative/mailto/tel/data — not checked.
    External,
    /// Same-page #fragment.
    Anchor,
}

pub fn classify_link(target: &str, from_source: &str, index: &dyn UrlIndex) -> LinkClass {
    // 1. Author-facing short-circuits.
    if target.starts_with("http://") || target.starts_with("https://")
        || target.starts_with("//") || target.starts_with("mailto:")
        || target.starts_with("tel:") || target.starts_with("data:")
    {
        return LinkClass::External;
    }
    if target.starts_with('#') {
        return LinkClass::Anchor;
    }

    let path = crate::resolve::fuzzy_path::split_url_path(target).0;
    if path.is_empty() {
        return LinkClass::Anchor; // pure ?query/#frag on current page
    }

    // 2. Asset-shaped (has a file extension) and not a known page → stay silent.
    let last = path.rsplit('/').next().unwrap_or(path);
    let asset_shaped = last.contains('.') && !last.ends_with('.');

    // 3. Absolute path: look up against the deployed URL space directly.
    // Note: trailing-slash differences (/research vs /research/) both Resolve — they redirect on hosts, they don't 404.
    if path.starts_with('/') {
        if index.lookup_exact(path.trim_start_matches('/')) || index.lookup_exact(path) {
            return LinkClass::Resolved { url: path.to_string() };
        }
        if let Some(canonical) = index.lookup_normalized(path) {
            return LinkClass::Mismatch { canonical };
        }
        if asset_shaped {
            return LinkClass::External; // silent
        }
        return LinkClass::Broken;
    }

    // 4. Reference (relative/bare): resolve to a canonical URL.
    if let Some(url) = index.resolve_reference_to_url(path, from_source) {
        return LinkClass::Resolved { url };
    }
    if asset_shaped {
        return LinkClass::External; // silent (relative asset the map doesn't index)
    }
    LinkClass::Broken
}

/// Backing data for classification, injected by the caller (zero-I/O in core).
pub trait UrlIndex {
    /// Case-sensitive presence of a URL path in the deployed space (host-accurate).
    /// Implementors MUST normalize `url_path` by stripping leading and trailing slashes before comparison; callers MAY pass paths with either or both.
    fn lookup_exact(&self, url_path: &str) -> bool;
    /// Case/slug-normalized match → canonical URL. MUST return Some only when the
    /// normalized bucket has exactly one member (else None — ambiguous).
    fn lookup_normalized(&self, url_path: &str) -> Option<String>;
    /// Resolve a wikilink/relative reference to its canonical URL path.
    fn resolve_reference_to_url(&self, reference: &str, from_source: &str) -> Option<String>;
}

/// Cross-module test fake for `UrlIndex` that returns the empty/negative result for every method.
/// Module-level so `reference.rs` tests can import it.
#[cfg(test)]
pub(crate) struct FakeUrlIndex;

#[cfg(test)]
impl FakeUrlIndex {
    pub fn new() -> Self { FakeUrlIndex }
}

#[cfg(test)]
impl UrlIndex for FakeUrlIndex {
    fn lookup_exact(&self, _url_path: &str) -> bool { false }
    fn lookup_normalized(&self, _url_path: &str) -> Option<String> { None }
    fn resolve_reference_to_url(&self, _reference: &str, _from_source: &str) -> Option<String> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linkclass_constructs() {
        assert_eq!(LinkClass::Broken, LinkClass::Broken);
    }


    // A tiny in-memory UrlIndex fixture:
    struct FakeIndex {
        exact: std::collections::HashSet<String>,
        normalized: std::collections::HashMap<String, Option<String>>, // norm_key -> Some(canonical)|None(ambiguous)
        refs: std::collections::HashMap<String, String>,               // reference -> url
    }
    impl UrlIndex for FakeIndex {
        fn lookup_exact(&self, u: &str) -> bool { self.exact.contains(u.trim_matches('/')) }
        fn lookup_normalized(&self, u: &str) -> Option<String> {
            self.normalized.get(&norm(u)).cloned().flatten()
        }
        fn resolve_reference_to_url(&self, r: &str, _from: &str) -> Option<String> {
            self.refs.get(r).cloned()
        }
    }
    fn norm(u: &str) -> String { u.trim_matches('/').to_lowercase() }

    fn idx() -> FakeIndex {
        let mut exact = std::collections::HashSet::new();
        exact.insert("research".to_string());
        let mut normalized = std::collections::HashMap::new();
        normalized.insert("research".to_string(), Some("/research/".to_string()));
        let mut refs = std::collections::HashMap::new();
        refs.insert("Research".to_string(), "/research/".to_string());
        FakeIndex { exact, normalized, refs }
    }

    #[test] fn external_passthrough() {
        assert_eq!(classify_link("https://x.com", "a.md", &idx()), LinkClass::External);
        assert_eq!(classify_link("mailto:a@b.c", "a.md", &idx()), LinkClass::External);
    }
    #[test] fn anchor_only() {
        assert_eq!(classify_link("#sec", "a.md", &idx()), LinkClass::Anchor);
    }
    #[test] fn absolute_exact_resolved() {
        assert_eq!(classify_link("/research/", "a.md", &idx()),
                   LinkClass::Resolved { url: "/research/".into() });
    }
    #[test] fn absolute_case_mismatch() { // the yinlab bug
        assert_eq!(classify_link("/Research/", "a.md", &idx()),
                   LinkClass::Mismatch { canonical: "/research/".into() });
    }
    #[test] fn absolute_mismatch_keeps_fragment_out_of_lookup() {
        assert_eq!(classify_link("/Research/#theme-1", "a.md", &idx()),
                   LinkClass::Mismatch { canonical: "/research/".into() });
    }
    #[test] fn reference_resolved() {
        assert_eq!(classify_link("Research", "a.md", &idx()),
                   LinkClass::Resolved { url: "/research/".into() });
    }
    #[test] fn asset_shaped_unknown_is_silent_not_broken() {
        // has an extension, not in index → treat as External (silent), never Broken
        assert_eq!(classify_link("/img/Logo.PNG", "a.md", &idx()), LinkClass::External);
    }
    #[test] fn unknown_reference_is_broken() {
        assert_eq!(classify_link("nope-no-page", "a.md", &idx()), LinkClass::Broken);
    }
}
