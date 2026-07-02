# Changelog

All notable changes to this crate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `resolve::md_extract`: pure markdown reference extractor (`extract_md_references`) returning every wikilink / embed / markdown-link / markdown-image token with byte offsets. Zero I/O; backs rename-with-references and delete-with-references in the editor.
- `frontmatter::strip_control_chars_str`: strips stray C0/C1 control characters (excluding TAB/LF/CR) from a string. Applied at the frontmatter write boundary as defense-in-depth against a macOS Tauri multiwebview bug that can type raw arrow-key control codes into text inputs (`tauri-apps/tauri#10194`); exposed `pub` so other crates' write paths can apply the same strip.

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
