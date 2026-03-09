# moss-core

Pure Rust content processing for [moss](https://github.com/nicholasgasior/moss). Zero I/O, zero async — takes strings in, returns strings out.

## Design Constraint

moss-core never touches the filesystem, network, or any async runtime. All I/O happens in the Tauri layer (`src-tauri/`). This keeps the library embeddable anywhere: Tauri commands, WASM, CLI tools, or test harnesses.

Every public function follows the same pattern: accept data (strings, parsed structs), return data (diagnostics, resolved content, parsed frontmatter).

## Modules

### `home` — Home file detection

Defines which filenames are recognized as a folder's index page: `index`, `readme`, `_index`, `main` (case-insensitive, priority order). This is the single source of truth for home file semantics — the Tauri layer, content graph, and SSG plugins all derive their behavior from `INDEX_STEMS`.

### `content_graph` — In-memory content index

`ContentGraph` is a read-only index of all content files, headings, and block IDs. It supports Obsidian-style fuzzy path resolution: exact path → filename-only → folder note (trying all home stems) → self-named folder note. Ambiguity is resolved by longest common directory prefix with the source file. Built incrementally via `ContentGraphBuilder`.

### `frontmatter` — YAML parsing with body preservation

Parses YAML frontmatter into `HashMap<String, serde_yaml::Value>` while preserving the markdown body byte-for-byte. Records the byte offsets of `---` delimiters so callers can do surgical replacement without re-serializing the entire file. Uses `serde_yaml` directly — not `gray_matter`, whose `Pod` type doesn't handle YAML arrays correctly (ADR-008).

### `heading_anchor` — Obsidian-compatible anchors

Converts heading text to URL anchors matching Obsidian's algorithm. Differs from standard slug generation: preserves parentheses, colons, commas, periods, ampersands; strips `#`, `^`, `|`, `[`, `]`, `\`; lowercases and collapses hyphens.

### `resolve` — Obsidian syntax resolution

Transforms Obsidian syntax to standard markdown before rendering. The pipeline:

1. Wikilink pass (`[[target]]` → `[target](url/)`)
2. Embed resolution (`![[file.md]]` → inlined content, with cycle detection)
3. Wikilink pass 2 (catch links inside embedded content)
4. Bare-filename image resolution (`![](photo.jpg)` → resolved path)
5. Block reference anchors (`^id` → `<span id="id">`)
6. Callout transformation (`> [!note]` → HTML)
7. Frontmatter wikilink resolution

All resolution goes through `ContentGraph` for path lookup. The module enforces an architectural boundary: no wikilink parsing or resolution happens outside `resolve`.

### `schema` — Content model definition

A custom JSON schema format (not JSON Schema) that defines frontmatter fields, types, and UI widget hints. The built-in schema is embedded at compile time via `include_str!`. Widget types include `TextInput`, `DatePicker`, `Select`, `TagInput`, `FilePicker`, and more. Designed to be language-agnostic — Rust, TypeScript, and WASM consumers all read the same schema. See ADR-008.

### `validation` — Schema-driven diagnostics

Validates parsed frontmatter against a `ContentSchema`, producing LSP-compatible diagnostics. Checks required fields, type mismatches, enum constraints, date format (strict YYYY-MM-DD with leap year logic), array item types, and unknown fields. Diagnostics include severity, message, field path, line, and column.

## Architecture

```
User's folder (markdown, images, videos)
        │
        ▼
   Tauri layer (I/O: scan files, read content, write output)
        │
        ├── moss-core (pure processing)
        │     ├── Parse frontmatter
        │     ├── Build content graph
        │     ├── Resolve wikilinks + embeds
        │     ├── Validate against schema
        │     └── Detect home files
        │
        ▼
   Page Tree (universal intermediate representation)
        │
        ├──→ Built-in generator (default, zero-config)
        ├──→ Hugo plugin (full theme ecosystem)
        ├──→ Astro plugin (component-based sites)
        └──→ Other SSG plugins
```

moss-core provides the building blocks that the Page Tree builder uses. The Page Tree is the contract between moss and all rendering engines — users learn one content model, and the generator is swappable. See `docs/architecture/ssg-plugin-architecture.md`.

## Content Model Decisions

These are settled — see the ADRs for rationale:

| Decision | Why | Reference |
|----------|-----|-----------|
| Custom schema format, not JSON Schema | Content model needs UI hints and extraction patterns, not just data shape validation | ADR-008 |
| `serde_yaml`, not `gray_matter` | gray_matter's `Pod` type doesn't deserialize YAML arrays into `HashMap<String, Value>` | ADR-008 |
| `include_str!` for built-in schema | Keeps moss-core filesystem-free | ADR-008 |
| Unified compilation pipeline | One `run_pipeline(config)` function; behavioral differences expressed in Config, not branching logic | ADR-010 |

## Consumer Rule

Every module must have a consumer before it ships. Tests prove a module works; a consumer proves it matters. Do not add modules to moss-core unless something in `src-tauri` or the frontend calls them. APIs change when they meet real usage.
