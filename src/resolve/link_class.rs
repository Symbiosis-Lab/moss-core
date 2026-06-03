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

/// Backing data for classification, injected by the caller (zero-I/O in core).
pub trait UrlIndex {
    /// Case-sensitive presence of a URL path in the deployed space (host-accurate).
    fn lookup_exact(&self, url_path: &str) -> bool;
    /// Case/slug-normalized match → canonical URL. MUST return Some only when the
    /// normalized bucket has exactly one member (else None — ambiguous).
    fn lookup_normalized(&self, url_path: &str) -> Option<String>;
    /// Resolve a wikilink/relative reference to its canonical URL path.
    fn resolve_reference_to_url(&self, reference: &str, from_source: &str) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linkclass_constructs() {
        assert_eq!(LinkClass::Broken, LinkClass::Broken);
    }
}
