//! Pure asset-reference resolver shared by editor + build (single source of truth).
//! Mirrors `link_class.rs`. Zero I/O — data injected via `AssetIndex` (ADR-018).

use crate::resolve::parent_dir;

pub trait AssetIndex {
    fn contains(&self, root_rel: &str) -> bool;
    fn contains_ci(&self, root_rel: &str) -> Option<String>;
    fn find_by_suffix(&self, suffix: &str) -> Vec<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetProvenance {
    Literal,
    BareFuzzy,
    SeparatorFallback,
    CaseMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetResolution {
    Resolved { root_rel: String, provenance: AssetProvenance },
    Ambiguous { chosen: String, candidates: Vec<String> },
    NotFound,
}

/// Lexically collapse `.`/`..` against a base dir; returns None if it escapes root.
fn lexical_join(base_dir: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base_dir.is_empty() { vec![] } else { base_dir.split('/').collect() };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop()?; } // pop; underflow → escape → None
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

#[allow(dead_code)]
fn has_separator(t: &str) -> bool {
    t.contains('/')
}

pub fn resolve_asset_ref(target: &str, from_source: &str, index: &impl AssetIndex) -> AssetResolution {
    // (filled out across Tasks 2-6)
    let from_dir = parent_dir(from_source);
    if let Some(cand) = lexical_join(from_dir, target) {
        if index.contains(&cand) {
            return AssetResolution::Resolved { root_rel: cand, provenance: AssetProvenance::Literal };
        }
    }
    AssetResolution::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Fake index over a fixed real-case path set (mirrors link_class.rs::FakeIndex).
    struct FakeIndex(HashSet<String>);
    impl FakeIndex {
        fn new(paths: &[&str]) -> Self {
            FakeIndex(paths.iter().map(|s| s.to_string()).collect())
        }
    }
    impl AssetIndex for FakeIndex {
        fn contains(&self, p: &str) -> bool {
            self.0.contains(p)
        }
        fn contains_ci(&self, p: &str) -> Option<String> {
            let lp = p.to_lowercase();
            self.0.iter().find(|x| x.to_lowercase() == lp).cloned()
        }
        fn find_by_suffix(&self, s: &str) -> Vec<String> {
            let ls = s.to_lowercase();
            let mut v: Vec<String> = self.0.iter()
                .filter(|x| x.to_lowercase().ends_with(&ls)
                    && (x.len() == s.len() || x.as_bytes()[x.len() - s.len() - 1] == b'/'))
                .cloned().collect();
            v.sort();
            v
        }
    }

    #[test]
    fn literal_exact_relative_hit() {
        let idx = FakeIndex::new(&["assets/Hoon.JPG", "team/photo.jpg"]);
        // ./photo.jpg authored next to team/Team.md → exists at team/photo.jpg
        let r = resolve_asset_ref("./photo.jpg", "team/Team.md", &idx);
        assert_eq!(r, AssetResolution::Resolved {
            root_rel: "team/photo.jpg".into(), provenance: AssetProvenance::Literal });
    }
}
