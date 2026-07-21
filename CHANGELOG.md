# Changelog

All notable changes to this crate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed
- **BREAKING** — `PageKind::Asset` is gone. It described a synthetic page generated per non-markdown file (the per-image pages an image-only folder produced). That generator was removed in 2026-07 because it could not tell an illustrations folder from a gallery — a folder of 83 article illustrations became 83 standalone pages titled with raw UUIDs. Nothing constructs the variant any more, so the enum no longer advertises a state that cannot occur, and `is_listable_at_depth_all` / `is_listable_at_depth_direct` lose an arm each. `PageKind` is not serialized into any on-disk artifact, so there is no stored data to migrate. **This is a breaking API change: the next release must bump the minor (0.1.x → 0.2.0) and update the workspace `moss-core` version constraint.**

### Added
- LaTeX math parsing, off by default. `ParseConfig::math` gates pulldown-cmark's `ENABLE_MATH`, so `$…$` and `$$…$$` become `Inline::Other` nodes rendering the equation's own source — **`$` delimiters included** — HTML-escaped, in `<code class="moss-math" data-moss-math="inline|display">`. The delimiters are part of the payload because P1 has no typesetting engine: the span is what the reader sees, so emitting only the inner TeX would silently delete two characters of the author's prose. That is not hypothetical — `一个$5，两个$10` parses as math (a full-width comma is non-whitespace, so it closes the span) and would have reached readers as `一个5，两个10`. A genuine equation therefore shows a visible `$…$` until a typesetting phase replaces the span. There is no typesetting engine in moss-core (ADR-030 keeps it app-side); this is the honest-fallback layer a renderer later upgrades to SVG. Existing callers are unaffected: `ParseConfig::default()` leaves math off, `$` stays an ordinary prose character, and no committed fixture changes.
  - Enabling math **without** handling both events silently deletes every equation — `Energy $E = mc^2$.` parsed to `<p>Energy .</p>`, because the AST walker's catch-all arms drop unrecognized inline events. The handler arms and the `parse_inline_event` whitelist entry that prevent this are covered by `tests/math_parsing.rs`, including math nested in list items — the wiring path a paragraph-only test cannot see, since `collect_item_blocks` is that whitelist's only caller. Math in table cells, blockquotes, links, callout bodies and footnotes reaches the parser by other routes and is covered separately.
  - `tests/fixtures/math-delimiters.vectors.json` records pulldown's `$` open/close rules as measured golden vectors — the cross-language contract a future editor grammar must agree with. The rule that decides every false positive: a closing `$` must be immediately preceded by a non-whitespace byte. `$5 and $10` survives only because the second `$` follows a *space* — that is a habit of English typography, not a property of ASCII or of currency. Put any punctuation between two prices and the span closes: `$5-$10` (price range), `$5/$10` (tiers) and `$9.99 ($8.99 for members)` (parenthetical) all parse as math, in English prose, on a default site. Unspaced CJK (`一个$5，两个$10`) parses for the same reason — a full-width comma is non-whitespace. In every case the text stays byte-exact, because the fallback keeps the delimiters; what changes is *presentation*, since the run renders in a monospace `<code>` span on published pages and in RSS item content. Any site whose prose puts two `$` in one paragraph without a space before the second should set `math = false` or escape as `\$`.
  - **Upgrade note — headings whose math contains `*`.** `[site].math` defaults to on, so upgrading enables math for sites that never set the key. Heading anchors normally survive that flip untouched, because the slug is computed from the equation's restored source. They do **not** survive when the TeX contains markdown-active characters: with math off, intraword `*` is eaten as emphasis first, so `# Convolution $f*g$ and $h*k$ end` was published as `convolution-$fg$-and-$hk$-end` and becomes `convolution-$f*g$-and-$h*k$-end`. `# Dual $V^*$ and $W^*$ end` moves the same way, and dual-space notation is common. Where this fires, external deep links, in-page TOC entries and `[[Page#Heading]]` wikilinks pointing at the old anchor break. Set `math = false` to keep the published anchors. See ADR-030 §"Upgrade-time anchor movement".
- `ast::parser_options(math: bool)`: the single constructor for moss's pulldown-cmark option set (strikethrough, tables, footnotes, wikilinks, plus math on request). Parser construction had drifted across five independent sites; call this instead of assembling `Options` by hand, and where a site genuinely needs a different set, call it and remove the option explicitly so the divergence stays visible.
- `link_completions::rank_completions`: deterministic ranking for wikilink/embed completion candidates. Sort keys: trigger-kind fit, prefix-at-start match quality, same-language-tree as `from_file`, directory-tree proximity, insert-value length, then lexicographic tiebreaks — mirroring `content_graph`'s resolver tiebreak chain so the dropdown surfaces the candidate a `[[link]]` would actually resolve to.
- `resolve::md_extract`: pure markdown reference extractor (`extract_md_references`) returning every wikilink / embed / markdown-link / markdown-image token with byte offsets. Zero I/O; backs rename-with-references and delete-with-references in the editor.
- `frontmatter::strip_control_chars_str`: strips stray C0/C1 control characters (excluding TAB/LF/CR) from a string. Applied at the frontmatter write boundary as defense-in-depth against a macOS Tauri multiwebview bug that can type raw arrow-key control codes into text inputs (`tauri-apps/tauri#10194`); exposed `pub` so other crates' write paths can apply the same strip.

### Fixed
- Section-scoped embeds with a human-readable anchor (e.g. `![[file#Heading With Spaces]]`) now slugify the target before matching, so they inline just the referenced section. Previously the raw anchor was compared against slugified headings, never matched, emitted a spurious `Heading '#...' not found` diagnostic, and fell back to inlining the entire file. Anchors already in slug form are unaffected.

### Changed
- `ComponentEntry::is_public()` now also returns `false` for internal implementation classes (`moss-apply*`), hiding them from `moss describe` and `docs/contract/reference.md`.
- Subscribe-form contract: `moss-subscribe-form` now always uses `data-position="inline"` (the auto-injected email footer and the `:::subscribe` shortcode emit identical HTML; footer vs in-page styling keys on the `footer` ancestor). Adds a `data-button-override` attribute, emitted when an author overrides the button label (`:::subscribe{button="…"}`); on such forms `subscribe.ts` leaves both the button label and the placeholder as authored instead of overwriting them with the language default. The former `data-position="footer"` value and the `footer-shape` slot / `data-moss-shape` footer attribute are removed.

_Pending publish — cumulative since `0.1.0` (last released on main)._
- Parse `color=` pipe attribute into `MediaAttrs` for cover-color override (repeated `color=` is last-wins).
- Parse `loop=` pipe attribute into `MediaAttrs` for ambient looping video (emits `data-loop`); add the `moss-ambient-video` / `moss-ambient-toggle` component-contract entries for the JS-injected pause/play affordance.
- Extend `moss describe --json` to schema **v5**: plugin hook contract (`plugin_hooks`), manifest fields (`manifest_fields`), slots, and CLI commands; `capabilities.required` now defaults to `false`; document the `-r|--recursive` import argument.
- Resolve folder references inside a language tree to the sibling `<lang>/<folder>/` index before falling back to the root.
- Parse nested `::::buttons` containers and `+++` thematic-break dividers (shortcode-structure parity corpus extended).
- Resolve bare `![[note]]` wikilink embeds as transclusions.
- **BREAKING:** Removed the `unlisted` frontmatter field. Use `draft` instead — a draft page now renders and is published at its direct URL but is hidden from all listings, feeds, sitemap, and navigation, and is marked `noindex`.
- Plus the [0.1.1] changes below (`home` marker, unified `classify_reference`, reference resolution moved into moss-core).

## [0.1.1] - 2026-06-11

### Added
- `home: true` frontmatter marker field; `re-key home override` decoupled from `translationKey`.
- `classify_reference`: `Link` fallthrough for unresolved unknown-extension references.
- `is_embed` gate on `classify_reference`; non-embed references resolve as `Link`.
- `resolve_path_with_overrides` and `reference_output_url` moved into moss-core (previously Tauri-only).
- `reference_kind_for_ext`: unified extension-to-kind table (replaces scattered `synth_kind_for_ext` calls).

### Fixed
- Validation no longer errors on `skip_schema` (internal/reserved) fields.
- Gate implicit-figure promotion to image-kind wikilinks only (prevents text wikilinks from becoming figures).
- Root homepage lists only its default-language tree in multilingual sites.

## [0.1.0] - 2026-05-29

### Added
- Initial publication to crates.io via the moss open-source release pipeline.
- Pure-Rust content engine: AST, render, resolve, validate, frontmatter, schema.
