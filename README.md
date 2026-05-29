# moss-core

Pure-Rust content engine for moss: AST, render, resolve, validate, frontmatter, schema. Zero I/O, zero async.

> **Read-only mirror** of `crates/moss-core/` in the private moss monorepo. Issues accepted here; PRs cannot be merged — see [CONTRIBUTING.md](CONTRIBUTING.md). PRs against the monorepo are the route for changes.

## Installation

```sh
cargo add moss-core
```

## Example

```rust
use moss_core::frontmatter;

fn main() {
    let source = "---\ntitle: Hello\ndate: 2026-01-01\n---\n\nBody text here.";

    let doc = frontmatter::parse(source);

    if let Some(title) = doc.frontmatter.get("title") {
        println!("Title: {title}");
    }
    println!("Body: {}", doc.body.trim());
}
```

`frontmatter::parse` is zero-allocation on the happy path: it splits at the `---` delimiters and deserializes only the YAML block. The body is preserved byte-for-byte.

## Modules

- **`frontmatter`** — YAML frontmatter parsing with body preservation and byte-offset tracking for surgical replacement.
- **`content_graph`** — In-memory index of all content files, headings, and block IDs. Supports Obsidian-style fuzzy path resolution.
- **`resolve`** — Transforms Obsidian syntax (wikilinks, embeds, callouts, block references) to standard Markdown before rendering.
- **`schema`** — Content model definition with UI widget hints. The built-in schema is the single source of truth for all frontmatter fields.
- **`validation`** — Schema-driven diagnostics: type checks, required fields, date format validation, enum constraints.
- **`heading_anchor`** — Obsidian-compatible heading-to-anchor conversion.
- **`ast`** — Markdown AST utilities built on `pulldown-cmark`.

## Documentation

Full API reference at <https://docs.rs/moss-core>.

## License

MIT — see [LICENSE](LICENSE).
