# Changelog

All notable changes to this crate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
