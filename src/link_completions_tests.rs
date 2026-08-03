use super::*;

fn cand(insert: &str, kind: CandidateKind) -> CompletionCandidate {
    CompletionCandidate {
        insert: insert.to_string(),
        label: insert.to_string(),
        rel_path: format!("{insert}.x"),
        kind,
    }
}

/// Candidate with an explicit project-relative path, for language/proximity
/// ranking tests.
fn cand_at(insert: &str, rel_path: &str, kind: CandidateKind) -> CompletionCandidate {
    CompletionCandidate {
        insert: insert.to_string(),
        label: insert.to_string(),
        rel_path: rel_path.to_string(),
        kind,
    }
}

#[test]
fn empty_prefix_returns_all_candidates() {
    let cands = vec![
        cand("about", CandidateKind::Page),
        cand("photo.png", CandidateKind::Asset),
    ];
    let ranked = rank_completions("", &cands, false, "");
    assert_eq!(ranked.len(), 2);
    // Link mode (embed=false): page ranks before asset.
    assert_eq!(cands[ranked[0]].kind, CandidateKind::Page);
    assert_eq!(cands[ranked[1]].kind, CandidateKind::Asset);
}

#[test]
fn prefix_filters_and_starts_with_ranks_first() {
    let cands = vec![
        cand("changelog", CandidateKind::Page), // contains "ang" in middle
        cand("angle", CandidateKind::Page),     // starts with "ang"
        cand("about", CandidateKind::Page),     // no match
    ];
    let ranked = rank_completions("ang", &cands, false, "");
    // "about" filtered out; "angle" (starts-with) before "changelog".
    assert_eq!(ranked.len(), 2);
    assert_eq!(cands[ranked[0]].insert, "angle");
    assert_eq!(cands[ranked[1]].insert, "changelog");
}

#[test]
fn case_insensitive_match() {
    let cands = vec![cand("README", CandidateKind::Page)];
    assert_eq!(rank_completions("read", &cands, false, "").len(), 1);
}

#[test]
fn embed_ranks_assets_before_pages() {
    let cands = vec![
        cand("hero", CandidateKind::Page),
        cand("hero.png", CandidateKind::Asset),
    ];
    // Both match "hero". With embed=true the asset must come first.
    let ranked = rank_completions("hero", &cands, true, "");
    assert_eq!(cands[ranked[0]].kind, CandidateKind::Asset);
    // With embed=false the page comes first.
    let ranked2 = rank_completions("hero", &cands, false, "");
    assert_eq!(cands[ranked2[0]].kind, CandidateKind::Page);
}

#[test]
fn cjk_prefix_matches() {
    let cands = vec![
        cand("刘果的笔记", CandidateKind::Page),
        cand("about", CandidateKind::Page),
    ];
    let ranked = rank_completions("刘果", &cands, false, "");
    assert_eq!(ranked.len(), 1);
    assert_eq!(cands[ranked[0]].insert, "刘果的笔记");
}

#[test]
fn heading_candidates_rank_starts_with_before_contains() {
    // A homogeneous Heading list: kind-priority is irrelevant (every
    // candidate is a Heading), so ranking degenerates to prefix-at-start
    // beating prefix-in-middle, then shorter, then lexicographic.
    let cands = vec![
        cand("Background and context", CandidateKind::Heading), // "context" in middle
        cand("Context", CandidateKind::Heading),                // starts with "context"
        cand("Conclusion", CandidateKind::Heading),             // no match
    ];
    let ranked = rank_completions("context", &cands, false, "");
    assert_eq!(ranked.len(), 2);
    assert_eq!(cands[ranked[0]].insert, "Context");
    assert_eq!(cands[ranked[1]].insert, "Background and context");
}

#[test]
fn heading_embed_flag_does_not_reorder_headings() {
    // Headings are completed for both `[[#` and (hypothetically) `![[#`;
    // the embed flag must not perturb a homogeneous heading list, since
    // no Heading is "the embed kind".
    // Equal-length labels so the length tiebreak is neutral and the
    // lexicographic tiebreak decides ("aaaa" < "bbbb").
    let cands = vec![
        cand("bbbb", CandidateKind::Heading),
        cand("aaaa", CandidateKind::Heading),
    ];
    let with_embed = rank_completions("", &cands, true, "");
    let without = rank_completions("", &cands, false, "");
    assert_eq!(with_embed, without);
    assert_eq!(cands[with_embed[0]].insert, "aaaa");
}

#[test]
fn nfc_and_nfd_forms_match_each_other() {
    // "café": NFC is U+00E9 (é as one codepoint); NFD is "cafe" + U+0301
    // (combining acute). A filename written in NFD must still match an NFC
    // prefix and vice versa — matching ContentGraph's normalize_component
    // so the suggestion resolves to the same target the link will.
    let nfc = "caf\u{00e9}"; // café (single é)
    let nfd = "cafe\u{0301}"; // café (e + combining accent)
    assert_ne!(nfc, nfd, "precondition: the two byte-forms differ");

    // NFD candidate, NFC prefix → matches.
    let cands = vec![cand(nfd, CandidateKind::Page)];
    assert_eq!(rank_completions(nfc, &cands, false, "").len(), 1);

    // NFC candidate, NFD prefix → matches.
    let cands2 = vec![cand(nfc, CandidateKind::Page)];
    assert_eq!(rank_completions(nfd, &cands2, false, "").len(), 1);
}

// ── Language-tree + tree-proximity ranking (uses from_file) ──────────

#[test]
fn same_language_tree_ranks_before_other_language() {
    // Two equally-good "guide" matches. From a zh-hans source, the zh-hans
    // candidate must win over the en one.
    let cands = vec![
        cand_at("guide", "en/guide.md", CandidateKind::Page),
        cand_at("guide", "zh-hans/guide.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("guide", &cands, false, "zh-hans/about.md");
    assert_eq!(cands[ranked[0]].rel_path, "zh-hans/guide.md");
}

#[test]
fn closer_in_tree_ranks_before_farther_in_same_language() {
    // Both same language (zh-hans), both match "note". From
    // zh-hans/游记/index.md the sibling under .../游记/ outranks the one at
    // the language-tree root.
    let cands = vec![
        cand_at("note", "zh-hans/note.md", CandidateKind::Page),
        cand_at("note", "zh-hans/游记/note.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("note", &cands, false, "zh-hans/游记/index.md");
    assert_eq!(cands[ranked[0]].rel_path, "zh-hans/游记/note.md");
}

#[test]
fn match_quality_outranks_language() {
    // Match quality (starts-with vs substring) is a higher-priority key than
    // language — a completion-specific choice (the resolver has no partial
    // match). A starts-with match in another language beats a middle-
    // substring match in the same one.
    let cands = vec![
        cand_at("周报report", "zh-hans/周报report.md", CandidateKind::Page), // same lang, middle
        cand_at("report-en", "en/report-en.md", CandidateKind::Page),        // other lang, starts
    ];
    let ranked = rank_completions("report", &cands, false, "zh-hans/about.md");
    assert_eq!(cands[ranked[0]].rel_path, "en/report-en.md");
}

#[test]
fn root_source_prefers_root_candidate_over_language_tree() {
    // A tree-less (root) source prefers a tree-less candidate — the "both
    // tree-less" arm of the language match, mirroring the resolver.
    let cands = vec![
        cand_at("about", "zh-hans/about.md", CandidateKind::Page),
        cand_at("about", "about.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("about", &cands, false, "index.md");
    assert_eq!(cands[ranked[0]].rel_path, "about.md");
}

#[test]
fn ties_break_by_path_deterministically() {
    // Same stem, same language, same proximity, same length → the resolver's
    // terminal key (alphabetically-smaller normalized path) decides, so the
    // result is independent of candidate walk order.
    let cands = vec![
        cand_at("guide", "zh-hans/b/guide.md", CandidateKind::Page),
        cand_at("guide", "zh-hans/a/guide.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("guide", &cands, false, "zh-hans/x.md");
    assert_eq!(cands[ranked[0]].rel_path, "zh-hans/a/guide.md");
}

#[test]
fn language_and_proximity_do_not_perturb_a_single_language_vault() {
    // When every candidate shares the source's language (or there is none),
    // the language/proximity keys tie and ordering falls back to the
    // existing match-quality/length/lexicographic behavior.
    let cands = vec![
        cand_at("changelog", "en/changelog.md", CandidateKind::Page), // "ang" middle
        cand_at("angle", "en/angle.md", CandidateKind::Page),         // "ang" start
    ];
    let ranked = rank_completions("ang", &cands, false, "en/index.md");
    assert_eq!(cands[ranked[0]].insert, "angle");
}
