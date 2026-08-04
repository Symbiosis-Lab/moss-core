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
///
/// A prefix containing a `/` (or `\`) is PATH-QUALIFIED: its segments are
/// matched, in order, against the candidate's full path components rather than
/// against the filename alone. That is what makes `關於/頭像-李` find
/// `關於/assets/頭像-李柏萱.png` even though the author omits the `assets/`
/// segment they never type. See [`insert_for`] for what such an accept inserts.
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
    let query = parse_query(prefix);

    // ONE normalization pass per candidate. Both the filter (`match_query`) and
    // the sort key (`score`) read the same `Prepared`, so neither `norm` nor
    // `normalize_path` runs twice — and `norm(prefix)` no longer runs per
    // candidate at all. Net per-keystroke work is lower than the filename-only
    // version this replaced, despite the added path logic.
    let mut hits: Vec<(Prepared<'_>, Hit)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let p = Prepared {
                idx: i,
                c,
                insert_norm: norm(&c.insert),
                cand_norm: crate::content_graph::normalize_path(&c.rel_path),
            };
            let hit = match_query(&query, &p)?;
            Some((p, hit))
        })
        .collect();
    // `sort_by_cached_key`, not `sort_by_key`: `score` allocates two Strings and
    // a Vec, so caching one key per element avoids re-running it on every
    // comparison across the whole-vault list.
    hits.sort_by_cached_key(|(p, hit)| score(&query, p, *hit, embed, from_lang, &from_dirs));
    hits.into_iter().map(|(p, _)| p.idx).collect()
}

/// A candidate with its two normalized forms computed once. `cand_norm`'s
/// components are re-split on demand rather than stored: a `Vec<&str>` borrowed
/// from a sibling field would make this struct self-referential.
struct Prepared<'a> {
    idx: usize,
    c: &'a CompletionCandidate,
    /// `norm(&c.insert)` — the bare filename / page name / heading text.
    insert_norm: String,
    /// `content_graph::normalize_path(&c.rel_path)` — the root-relative path.
    cand_norm: String,
}

/// The typed prefix, split into path segments.
///
/// `path_qualified` is what switches the matcher from "filename contains" to
/// "ordered subsequence over path components". Empty segments are dropped, so a
/// leading `/` is harmless (candidates are root-relative already) and `a//b` is
/// `a`,`b`. `dir_only` (a trailing `/`) restricts matching to directories.
struct Query {
    segs: Vec<String>,
    path_qualified: bool,
    dir_only: bool,
}

fn parse_query(prefix: &str) -> Query {
    let raw = prefix.replace('\\', "/");
    Query {
        path_qualified: raw.contains('/'),
        dir_only: raw.ends_with('/'),
        segs: raw.split('/').filter(|s| !s.is_empty()).map(norm).collect(),
    }
}

/// How well a candidate matched — two sort keys, both constant `0` for a
/// non-path-qualified query (see `match_query`'s single early return), which is
/// what makes bare-query ordering provably identical to before.
#[derive(Debug, Clone, Copy)]
struct Hit {
    /// 0 = the LAST query segment matched the filename component; 1 = it only
    /// matched a directory (so `關於/ass` still lists the subtree while any true
    /// filename match outranks it).
    seg_hit: u8,
    /// 0 = the matched components are contiguous AND end at the filename (a true
    /// path-suffix match); 1 = gapped. Mirrors `resolve_asset_ref` step 4, which
    /// tries `find_by_suffix(target)` before `find_by_suffix(basename)`.
    dir_tight: u8,
}

/// Filter + match-quality, in one pass. `None` drops the candidate.
fn match_query(q: &Query, p: &Prepared<'_>) -> Option<Hit> {
    if q.segs.is_empty() {
        return Some(Hit { seg_hit: 0, dir_tight: 0 });
    }
    // Headings are excluded from path logic on purpose: heading TEXT may contain
    // `/` (`[[Page#A/B]]`), and a heading candidate's `rel_path` is the literal
    // string `"H2"`, so splitting the query would match the user's words against
    // a level marker. A non-path-qualified query takes the same branch, which is
    // today's rule verbatim.
    if !q.path_qualified || p.c.kind == CandidateKind::Heading {
        return if p.insert_norm.contains(&q.segs[0]) {
            Some(Hit { seg_hit: 0, dir_tight: 0 })
        } else {
            None
        };
    }

    let comps: Vec<&str> = p.cand_norm.split('/').filter(|s| !s.is_empty()).collect();
    if comps.is_empty() {
        return None;
    }
    let file_i = comps.len() - 1;
    // A trailing `/` means "inside this directory": the segments must all be
    // consumed by directory components, never by the filename.
    let limit = if q.dir_only { file_i } else { comps.len() };

    let mut positions: Vec<usize> = Vec::with_capacity(q.segs.len());
    let mut next = 0usize;
    for seg in &q.segs {
        let mut found = None;
        while next < limit {
            let at = next;
            next += 1;
            if comps[at].contains(seg.as_str()) {
                found = Some(at);
                break;
            }
        }
        positions.push(found?);
    }

    // `?` rather than `expect`: the crate denies `clippy::expect_used`, and the
    // empty case is already handled by the early return above — so this is a
    // restatement of an invariant, not a real branch.
    let last = *positions.last()?;
    let contiguous = positions.windows(2).all(|w| w[1] == w[0] + 1);
    Some(Hit {
        seg_hit: u8::from(last != file_i),
        dir_tight: u8::from(!(contiguous && last == file_i)),
    })
}

/// The reference text an accepted completion must insert, or `None` when the
/// bare `insert` value is already correct (a non-path-qualified query, or a
/// `Heading`).
///
/// THE INVARIANT: the emitted form is the one whose FIRST applicable resolver
/// step reproduces `c.rel_path` exactly.
///
/// - `Page` → `rel_path` minus `.md`, root-relative, no leading `/`.
///   [`crate::content_graph::ContentGraph::resolve_path`] has no source-relative
///   step, so its exact `path_index` lookup (step 1/2) pins this regardless of
///   where the source file lives.
/// - `Asset` inside the source's own subtree → the path relative to
///   `parent_dir(from_rel)`. `resolve_asset_ref` STEP 2 (`lexical_join`) then
///   reproduces `rel_path` by construction → `Literal`.
/// - `Asset` anywhere else → `"/" + rel_path`, pinned by step 1.
///
/// The bare root-relative asset form is never emitted: from `關於/x.md` it would
/// hit step 2 first and silently resolve `assets/hero.png` to an entirely
/// different, existing `關於/assets/hero.png`. `..` is never emitted either —
/// step-1 anchoring is strictly simpler and equally exact.
///
/// Extension policy lives here, next to the ranker that owns it: pages drop
/// `.md`, assets keep their extension.
pub fn insert_for(prefix: &str, c: &CompletionCandidate, from_rel: &str) -> Option<String> {
    let q = parse_query(prefix);
    if !q.path_qualified || q.segs.is_empty() {
        return None;
    }
    let rel = c.rel_path.replace('\\', "/");
    match c.kind {
        CandidateKind::Heading => None,
        CandidateKind::Page => Some(rel.strip_suffix(".md").unwrap_or(&rel).to_string()),
        CandidateKind::Asset => {
            let from = from_rel.replace('\\', "/");
            let from_dir = crate::resolve::parent_dir(&from);
            Some(source_relative(from_dir, &rel).unwrap_or_else(|| format!("/{rel}")))
        }
    }
}

/// `rel_path` expressed relative to `from_dir`, or `None` when it is not inside
/// that directory. Compares COMPONENTS, never the raw string — `關於2/x.png`
/// must not count as living inside `關於`. Never produces a `..` segment.
fn source_relative(from_dir: &str, rel_path: &str) -> Option<String> {
    let base: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let target: Vec<&str> = rel_path.split('/').filter(|s| !s.is_empty()).collect();
    if target.len() <= base.len() {
        return None;
    }
    if base.iter().enumerate().any(|(i, b)| target[i] != *b) {
        return None;
    }
    Some(target[base.len()..].join("/"))
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

/// Lower score sorts first. Ordering, in priority:
/// 1. kind matches the trigger (`embed` → Asset first, else Page first)
/// 2. prefix-at-start beats prefix-in-middle (match quality)
/// 3. the last query segment matched the FILENAME, not just a directory
/// 4. the matched components are a contiguous path suffix, not a gapped one
/// 5. same language tree as the source (or both tree-less) beats a different one
/// 6. closer in the directory tree (longer shared dir prefix) beats farther
/// 7. shorter insert value (closer match) beats longer
/// 8. lexicographic insert value
/// 9. lexicographic normalized path (fully deterministic, independent of the
///    filesystem walk order)
///
/// Keys 5, 6 and 9 mirror the resolver's tiebreak chain in
/// [`crate::content_graph`] (`tree_match`, `common_prefix_len`,
/// `Reverse(normalized path)`), so when two candidates share a stem the
/// dropdown surfaces the same one the link would resolve to. Keys 1–4 are
/// completion-specific — the resolver matches exact stems and has no notion of
/// trigger kind or partial-match quality.
///
/// Keys 3 and 4 are inserted BEFORE that resolver-mirroring chain begins and are
/// the constant `0` for every candidate when the query has no separator, so they
/// cannot perturb bare-query order (see `bare_query_ordering_is_unchanged_by_path_keys`).
/// Key 2 is computed against the query's LAST segment, which for a bare query is
/// the whole prefix — byte-identical to before.
///
/// `from_lang` / `from_dirs` are the source file's language-tree prefix and
/// directory components (computed once by the caller).
fn score(
    q: &Query,
    p: &Prepared<'_>,
    hit: Hit,
    embed: bool,
    from_lang: Option<&str>,
    from_dirs: &[&str],
) -> (u8, u8, u8, u8, u8, Reverse<usize>, usize, String, String) {
    let c = p.c;
    let kind_rank = match (embed, c.kind) {
        (true, CandidateKind::Asset) | (false, CandidateKind::Page) => 0u8,
        _ => 1u8,
    };
    let starts = match q.segs.last() {
        Some(last) if p.insert_norm.starts_with(last.as_str()) => 0u8,
        _ => 1u8,
    };

    // Language tree + directory proximity, both relative to the source file and
    // computed on the resolver-normalized candidate path.
    let cand_lang = crate::home::lang_tree_prefix(&p.cand_norm);
    let lang_rank = match (from_lang, cand_lang) {
        (Some(f), Some(cc)) if f.eq_ignore_ascii_case(cc) => 0u8,
        (None, None) => 0u8,
        _ => 1u8,
    };
    let proximity = crate::content_graph::common_prefix_len(
        &crate::content_graph::dir_components(&p.cand_norm),
        from_dirs,
    );

    (
        kind_rank,
        starts,
        hit.seg_hit,
        hit.dir_tight,
        lang_rank,
        Reverse(proximity), // more shared dirs sorts first under ascending order
        c.insert.chars().count(), // scalar count, not byte len — CJK filenames sort correctly
        p.insert_norm.clone(),
        p.cand_norm.clone(), // terminal: alphabetical-by-normalized-path (smallest wins, mirroring the resolver's Reverse(path))
    )
}

#[cfg(test)]
#[path = "link_completions_tests.rs"]
mod tests;
