//! Pure ranker for wikilink autocomplete completions.
//!
//! moss-core is zero-I/O: the Tauri layer walks the source filesystem and
//! builds the candidate list; this module ranks it against a typed prefix.
//! Ranking mirrors the resolver's NFC-normalize + lowercase comparison so the
//! suggested target is the one the link resolver will actually resolve.

use std::cmp::Reverse;

use unicode_normalization::UnicodeNormalization;

/// What a completion candidate targets.
///
/// `Page`/`Asset` are the two kinds offered for the `[[`/`![[` page+asset walk
/// (their relative priority flips with the `embed` trigger). `Heading` is the
/// kind offered for `[[Page#…` heading completion — a homogeneous list where
/// every candidate is a heading, so kind-priority is irrelevant and ranking
/// degenerates to prefix/length/lexicographic ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Page,
    Asset,
    Heading,
}

/// One completable target, pre-computed by the Tauri layer.
#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    /// What goes inside the brackets: md filename WITHOUT `.md`, asset
    /// filename WITH extension.
    pub insert: String,
    /// Human-readable label shown in the dropdown (same as `insert` for v1).
    pub label: String,
    /// Project-root-relative path, shown as `detail` to disambiguate.
    pub rel_path: String,
    /// Page (markdown) vs Asset (image/video/etc.) — drives trigger-aware ranking.
    pub kind: CandidateKind,
}

/// Rank `candidates` against `prefix`, returning indices into `candidates`
/// ordered best-first. An empty prefix returns every candidate (kind-ordered).
/// `embed` (triggered by `![[`) ranks assets before pages; otherwise pages
/// rank first.
///
/// `from_file` is the project-relative path of the file the completion is being
/// typed in. It biases ties toward the source's own context — candidates in the
/// same language tree (e.g. both under `zh-hans/`), and then candidates closer
/// in the directory tree, rank higher. This mirrors [`crate::content_graph`]'s
/// resolver so the dropdown order matches how a link would actually resolve.
pub fn rank_completions(
    prefix: &str,
    candidates: &[CompletionCandidate],
    embed: bool,
    from_file: &str,
) -> Vec<usize> {
    // Fold the source path the same way the resolver does, then derive its
    // language tree and directory components once for every candidate to score
    // against.
    let from_norm = crate::content_graph::normalize_path(from_file);
    let from_lang = crate::home::lang_tree_prefix(&from_norm);
    let from_dirs = crate::content_graph::dir_components(&from_norm);

    let mut idx: Vec<usize> = (0..candidates.len()).collect();
    idx.retain(|&i| matches(prefix, &candidates[i].insert));
    // `sort_by_cached_key`, not `sort_by_key`: `score` normalizes the candidate
    // path (NFC + lowercase, plus a Vec alloc), so caching one key per element
    // avoids re-running it on every comparison across the whole-vault list.
    idx.sort_by_cached_key(|&i| score(prefix, &candidates[i], embed, from_lang, &from_dirs));
    idx
}

/// NFC-normalize then lowercase — identical to
/// [`crate::content_graph`]'s `normalize_component`, so a completion suggestion
/// folds the same way the link resolver will fold it at build time. Without the
/// NFC step a decomposed-form filename (e.g. NFD CJK/accented codepoints, which
/// HFS+ historically wrote and APFS preserves) could rank as a match here but
/// resolve differently in `ContentGraph`, suggesting a target that doesn't
/// round-trip.
fn norm(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

/// A candidate matches when its normalized insert value contains the
/// normalized prefix (empty prefix matches everything).
fn matches(prefix: &str, insert: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    norm(insert).contains(&norm(prefix))
}

/// Lower score sorts first. Ordering, in priority:
/// 1. kind matches the trigger (`embed` → Asset first, else Page first)
/// 2. prefix-at-start beats prefix-in-middle (match quality)
/// 3. same language tree as the source (or both tree-less) beats a different one
/// 4. closer in the directory tree (longer shared dir prefix) beats farther
/// 5. shorter insert value (closer match) beats longer
/// 6. lexicographic insert value
/// 7. lexicographic normalized path (fully deterministic, independent of the
///    filesystem walk order)
///
/// Keys 3, 4 and 7 mirror the resolver's tiebreak chain in
/// [`crate::content_graph`] (`tree_match`, `common_prefix_len`,
/// `Reverse(normalized path)`), so when two candidates share a stem the
/// dropdown surfaces the same one the link would resolve to. Keys 1–2 are
/// completion-specific — the resolver matches exact stems and has no notion of
/// trigger kind or partial-match quality.
///
/// `from_lang` / `from_dirs` are the source file's language-tree prefix and
/// directory components (computed once by the caller).
fn score(
    prefix: &str,
    c: &CompletionCandidate,
    embed: bool,
    from_lang: Option<&str>,
    from_dirs: &[&str],
) -> (u8, u8, u8, Reverse<usize>, usize, String, String) {
    let kind_rank = match (embed, c.kind) {
        (true, CandidateKind::Asset) | (false, CandidateKind::Page) => 0u8,
        _ => 1u8,
    };
    let starts = if !prefix.is_empty() && norm(&c.insert).starts_with(&norm(prefix)) { 0u8 } else { 1u8 };

    // Language tree + directory proximity, both relative to the source file and
    // computed on the resolver-normalized candidate path.
    let cand_norm = crate::content_graph::normalize_path(&c.rel_path);
    let cand_lang = crate::home::lang_tree_prefix(&cand_norm);
    let lang_rank = match (from_lang, cand_lang) {
        (Some(f), Some(cc)) if f.eq_ignore_ascii_case(cc) => 0u8,
        (None, None) => 0u8,
        _ => 1u8,
    };
    let proximity = crate::content_graph::common_prefix_len(
        &crate::content_graph::dir_components(&cand_norm),
        from_dirs,
    );

    (
        kind_rank,
        starts,
        lang_rank,
        Reverse(proximity), // more shared dirs sorts first under ascending order
        c.insert.chars().count(), // scalar count, not byte len — CJK filenames sort correctly
        norm(&c.insert),
        cand_norm, // terminal: alphabetical-by-normalized-path (smallest wins, mirroring the resolver's Reverse(path))
    )
}

#[cfg(test)]
#[path = "link_completions_tests.rs"]
mod tests;
