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

// ── Path-qualified queries (a `/` in the prefix) ─────────────────────

#[test]
fn path_query_matches_across_an_omitted_directory() {
    // THE CORPUS BUG (/tmp/frontline-test/在場.md:48 writes `關於/頭像-李柏萱.png`
    // for a file that actually lives at `關於/assets/頭像-李柏萱.png`). Before
    // path-awareness the ranker only ever compared the FILENAME, so any query
    // containing `/` matched nothing at all.
    let cands = vec![cand_at(
        "頭像-李柏萱.png",
        "關於/assets/頭像-李柏萱.png",
        CandidateKind::Asset,
    )];
    let ranked = rank_completions("關於/頭像-李", &cands, true, "在場.md");
    assert_eq!(ranked.len(), 1);
}

#[test]
fn path_query_rejects_a_different_directory() {
    let cands = vec![cand_at(
        "頭像-李柏萱.png",
        "關於/assets/頭像-李柏萱.png",
        CandidateKind::Asset,
    )];
    assert!(rank_completions("獎項/頭像-李", &cands, true, "在場.md").is_empty());
}

#[test]
fn partial_directory_segment_still_lists_the_subtree() {
    // Mid-typing a directory name (`關於/ass`) must NOT empty the dropdown —
    // that is the exact symptom being fixed. The final segment is allowed to
    // match a DIRECTORY component, and `seg_hit` keeps any real filename match
    // above the directory-only ones.
    let cands = vec![
        cand_at("f99cc68b.png", "關於/assets/f99cc68b.png", CandidateKind::Asset),
        cand_at("assembly.png", "關於/assembly.png", CandidateKind::Asset),
    ];
    let ranked = rank_completions("關於/ass", &cands, true, "在場.md");
    assert_eq!(ranked.len(), 2);
    // `assembly.png` matches on the FILENAME (seg_hit 0); the subtree listing
    // under `assets/` matched only a directory (seg_hit 1).
    assert_eq!(cands[ranked[0]].insert, "assembly.png");
}

#[test]
fn bare_query_ordering_is_unchanged_by_path_keys() {
    // The neutrality proof for `seg_hit`/`dir_tight`/the last-segment `starts`.
    // Same assertions as `prefix_filters_and_starts_with_ranks_first`, but with
    // directory-bearing rel_paths so the new keys would have something to bite
    // on if they were not constant for a separator-free query.
    // MUST NOT BE DELETED — nothing else holds this.
    let cands = vec![
        cand_at("changelog", "en/notes/changelog.md", CandidateKind::Page),
        cand_at("angle", "en/angle.md", CandidateKind::Page),
        cand_at("about", "en/about.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("ang", &cands, false, "en/index.md");
    assert_eq!(ranked.len(), 2);
    assert_eq!(cands[ranked[0]].insert, "angle");
    assert_eq!(cands[ranked[1]].insert, "changelog");
}

#[test]
fn contiguous_suffix_dir_match_outranks_a_gapped_one() {
    // `dir_tight` mirrors resolve_asset_ref step 4, which tries
    // find_by_suffix(target) before find_by_suffix(basename).
    let cands = vec![
        cand_at("x.png", "關於/deep/assets/x.png", CandidateKind::Asset), // gapped
        cand_at("x.png", "assets/x.png", CandidateKind::Asset),           // contiguous suffix
    ];
    let ranked = rank_completions("assets/x", &cands, true, "在場.md");
    assert_eq!(cands[ranked[0]].rel_path, "assets/x.png");
}

#[test]
fn starts_with_uses_the_final_segment_for_a_path_query() {
    // Without this the `starts` key is a constant 1 for every path query (no
    // filename starts with `dir/angle`) and the signal is lost.
    let cands = vec![
        cand_at("changelog", "dir/changelog.md", CandidateKind::Page),
        cand_at("angle", "dir/angle.md", CandidateKind::Page),
    ];
    let ranked = rank_completions("dir/ang", &cands, false, "index.md");
    assert_eq!(cands[ranked[0]].insert, "angle");
}

#[test]
fn heading_candidates_ignore_slashes_in_the_query() {
    // A heading's rel_path is the literal string "H2" and its TEXT may contain
    // `/`, so path logic must not apply. `insert_for` returns None so the caller
    // inserts the heading text, never "H2".
    let cands = vec![cand_at("Intro/Setup", "H2", CandidateKind::Heading)];
    let ranked = rank_completions("Intro/Set", &cands, false, "notes.md");
    assert_eq!(ranked.len(), 1);
    assert_eq!(insert_for("Intro/Set", &cands[0], "notes.md"), None);
}

#[test]
fn trailing_slash_lists_only_that_directory() {
    let cands = vec![
        cand_at("a.png", "關於/assets/a.png", CandidateKind::Asset),
        cand_at("b.png", "獎項/b.png", CandidateKind::Asset),
    ];
    let ranked = rank_completions("關於/", &cands, true, "在場.md");
    assert_eq!(ranked.len(), 1);
    assert_eq!(cands[ranked[0]].rel_path, "關於/assets/a.png");
}

// ── The insert_for invariant, enforced through the REAL resolvers ────

#[test]
fn insert_for_always_round_trips_through_the_resolver() {
    use crate::resolve::asset_class::{
        resolve_asset_ref, AssetProvenance, AssetResolution, FakeAssetIndex,
    };

    // The corpus shape: a root-level source and a nested one, against a sibling
    // asset, a subtree asset, a cross-tree asset and a root-level asset. The
    // cross-tree row is the collision case — `assets/首頁hero.png` seen from
    // `關於/x.md`, where the bare root-relative form would resolve to the
    // DIFFERENT, also-existing `關於/assets/首頁hero.png`.
    let paths = [
        "關於/assets/頭像-李柏萱.png",
        "關於/歷季得獎者.md",
        "關於/近照.png",
        "assets/首頁hero.png",
        "關於/assets/首頁hero.png",
        "首頁.png",
    ];
    let idx = FakeAssetIndex::new(&paths);

    for from_rel in ["在場.md", "關於/歷季得獎者.md"] {
        for rel in [
            "關於/assets/頭像-李柏萱.png",
            "關於/近照.png",
            "assets/首頁hero.png",
            "首頁.png",
        ] {
            let c = cand_at(
                rel.rsplit('/').next().unwrap(),
                rel,
                CandidateKind::Asset,
            );
            let emitted = insert_for("關於/x", &c, from_rel)
                .unwrap_or_else(|| panic!("path-qualified query must emit a form for {rel}"));
            assert_eq!(
                resolve_asset_ref(&emitted, from_rel, &idx),
                AssetResolution::Resolved {
                    root_rel: rel.to_string(),
                    provenance: AssetProvenance::Literal,
                },
                "from {from_rel}, candidate {rel}, emitted {emitted}"
            );
        }
    }

    // The page leg, through the resolver pages actually use.
    let mut b = crate::content_graph::ContentGraphBuilder::new();
    b.add_file("notes/ideas.md", "notes/ideas");
    b.add_file("關於/歷季得獎者.md", "關於/歷季得獎者");
    let graph = b.build();
    for from_rel in ["在場.md", "關於/歷季得獎者.md"] {
        let c = cand_at("ideas", "notes/ideas.md", CandidateKind::Page);
        let emitted = insert_for("notes/id", &c, from_rel).unwrap();
        assert_eq!(emitted, "notes/ideas");
        assert_eq!(graph.resolve_path(&emitted, from_rel).as_deref(), Some("notes/ideas.md"));
    }
}

#[test]
fn insert_for_is_none_without_a_separator() {
    // The chip-bar contract: a bare query never produces a path form, so the
    // frontmatter cover/logo picker keeps writing the bare filename.
    let c = cand_at("x.png", "關於/assets/x.png", CandidateKind::Asset);
    assert_eq!(insert_for("x", &c, "關於/y.md"), None);
}
