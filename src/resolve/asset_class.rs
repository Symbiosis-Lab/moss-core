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

fn has_separator(t: &str) -> bool {
    t.contains('/')
}

pub fn resolve_asset_ref(target: &str, from_source: &str, index: &impl AssetIndex) -> AssetResolution {
    let from_dir = parent_dir(from_source);

    // Step 1: `/`-absolute → root.
    if let Some(stripped) = target.strip_prefix('/') {
        return finish(stripped.to_string(), AssetProvenance::Literal, index);
    }

    // Step 2: literal source-relative (all targets).
    if let Some(cand) = lexical_join(from_dir, target) {
        if let Some(res) = finish_opt(&cand, AssetProvenance::Literal, index) { return res; }
    }

    // Step 3: project-root-relative — SEPARATOR targets only (bare handled in step 4).
    if has_separator(target) {
        if let Some(cand) = lexical_join("", target) {
            if let Some(res) = finish_opt(&cand, AssetProvenance::SeparatorFallback, index) { return res; }
        }
    }

    // Step 4 added in Task 3.
    AssetResolution::NotFound
}

/// Exact hit → Resolved(provenance); case-only hit → Resolved(CaseMismatch, canonical); else None.
fn finish_opt(cand: &str, prov: AssetProvenance, index: &impl AssetIndex) -> Option<AssetResolution> {
    if index.contains(cand) {
        return Some(AssetResolution::Resolved { root_rel: cand.to_string(), provenance: prov });
    }
    if let Some(canon) = index.contains_ci(cand) {
        return Some(AssetResolution::Resolved { root_rel: canon, provenance: AssetProvenance::CaseMismatch });
    }
    None
}
fn finish(cand: String, prov: AssetProvenance, index: &impl AssetIndex) -> AssetResolution {
    finish_opt(&cand, prov, index).unwrap_or(AssetResolution::NotFound)
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
    #[test]
    fn separator_fallback_to_root() {
        // ./assets/AGU2025.jpg from a subfolder; real file at root assets/
        let idx = FakeIndex::new(&["assets/AGU2025.jpg"]);
        let r = resolve_asset_ref("./assets/AGU2025.jpg", "News/2025-12-agu.md", &idx);
        assert_eq!(r, AssetResolution::Resolved {
            root_rel: "assets/AGU2025.jpg".into(),
            provenance: AssetProvenance::SeparatorFallback });
    }
    #[test]
    fn case_mismatch_on_literal() {
        // ./assets/Hoon.jpg authored at root; disk is Hoon.JPG
        let idx = FakeIndex::new(&["assets/Hoon.JPG"]);
        let r = resolve_asset_ref("./assets/Hoon.jpg", "Team.md", &idx);
        assert_eq!(r, AssetResolution::Resolved {
            root_rel: "assets/Hoon.JPG".into(),  // canonical real case
            provenance: AssetProvenance::CaseMismatch });
    }
}
