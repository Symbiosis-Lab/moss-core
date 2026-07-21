//! moss component contract — Source 2 of the federated contract.
//!
//! Single source of truth for every `moss-*` class moss currently emits.
//! Each entry declares: the class name, its kind (container/instance/standalone/chrome),
//! accepted `data-*` attributes with value spaces, example HTML, example markdown.
//!
//! ## Adding a new emitter class
//!
//! 1. Emit the class from your renderer module (`build/markdown/*`, `build/components/*`).
//! 2. Add a `ComponentEntry` to [`COMPONENTS`] here.
//! 3. Run `cargo test --test components_sync_test` from src-tauri/ — the
//!    scanner test will fail if you forget.
//! 4. Run `cargo run --bin generate-contract-docs --features dev-tools` to
//!    refresh `docs/contract/reference.md`.
//!
//! ## Why a const table, not a derive macro?
//!
//! Mirrors the BUILTIN_FIELDS precedent in `schema_fields.rs`. The synchronization
//! is enforced by a sync test (`emitter_classes_match_components_table`) that
//! scans emitter Rust source for `class="moss-..."` literals. This is a
//! best-effort scanner (won't catch classes assembled via `format!()`), not a
//! type-checked guarantee like BUILTIN_FIELDS' compile-time mirror. The
//! limitation is documented in the spec § Source 2.

/// Status of a component entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// In active use; theme authors can rely on it.
    Confirmed,
    /// Emerging convention; may evolve.
    Emerging,
    /// Scheduled for removal; theme authors should migrate.
    Retired,
}

/// A declared `data-*` attribute on a component.
pub struct DataAttr {
    /// Attribute name including `data-` prefix (e.g. `"data-layout"`).
    pub name: &'static str,
    /// Allowed values (e.g. `&["grid", "list", "minimal"]`). Empty means free-form.
    pub values: &'static [&'static str],
    /// Default value (first in `values`, or `""` for free-form).
    pub default: &'static str,
    /// Short description shown in reference.md.
    pub description: &'static str,
}

/// A single component contract entry.
pub struct ComponentEntry {
    /// Class name without leading `.` (e.g. `"moss-cards"`).
    pub class: &'static str,
    /// Container / Instance / Standalone / Chrome.
    pub kind: &'static str,
    /// For Instance kinds, the parent container's class (or `""`).
    pub parent: &'static str,
    /// Declared `data-*` attributes on the element with this class.
    pub data_attrs: &'static [DataAttr],
    /// Example HTML snippet showing the class in context. Multi-line allowed.
    pub example_html: &'static str,
    /// Example markdown that produces this HTML. Empty for HTML-only chrome.
    pub example_markdown: &'static str,
    /// Status: confirmed / emerging / retired.
    pub status: Status,
    /// Contract version this entry was introduced in.
    pub since: &'static str,
    /// Optional human-readable description.
    pub description: &'static str,
}

/// The full contract surface — every `moss-*` class moss currently emits.
///
/// Phase 0b seeds this with the CURRENT emitted vocabulary (not the
/// v1-collapsed shape). Phase 1c rewrites to the collapsed form.
pub const COMPONENTS: &[ComponentEntry] = &[
    ComponentEntry {
        class: "moss-cards",
        kind: "container",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-layout",
                values: &["grid", "list", "minimal"],
                default: "grid",
                description: "Card layout density. Grid: 2-3 cols with covers. List: single column with side covers. Minimal: text-only with year groupings.",
            },
            DataAttr {
                name: "data-density",
                values: &["default", "compact"],
                default: "default",
                description: "Vertical spacing density.",
            },
            DataAttr {
                name: "data-list-axis",
                values: &["date", "weight", "title"],
                default: "title",
                description: "Sort axis for the listing (mirrors the folder's `sort:` frontmatter). Drives `--moss-card-min` density tuning and decides whether each `.moss-card-meta` slot is filled (date axis) or omitted (weight/title axes).",
            },
            DataAttr {
                name: "data-list-has-covers",
                values: &[""],
                default: "",
                description: "Boolean presence flag: emitted iff any child card has a cover. Combines with `data-list-axis` to widen `--moss-card-min` for cover-led layouts. Use `[data-list-has-covers]` in CSS to target it.",
            },
        ],
        example_html: r#"<div class="moss-cards-container">
  <div class="moss-cards" data-layout="grid" data-list-axis="date" data-list-has-covers>
    <a class="moss-card" href="...">...</a>
    <a class="moss-card" href="...">...</a>
  </div>
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Auto-generated listing of child pages. The single canonical container; layout density on `data-layout` (`grid` for cover-led tiles, `list` for cover+excerpt rows, `minimal` for text-only year-grouped indexes). Wrapped in `.moss-cards-container` to scope CSS container queries.",
    },
    ComponentEntry {
        class: "moss-cards-container",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-cards-container">
  <div class="moss-cards" data-layout="grid">...</div>
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Outer wrapper around `.moss-cards` that carries `container-type: inline-size` so the grid can use `@container` queries instead of viewport `@media` queries. Layout-agnostic — wraps any `data-layout` variant.",
    },
    ComponentEntry {
        class: "moss-summary-layout",
        kind: "container",
        parent: "moss-cards",
        data_attrs: &[],
        example_html: r#"<div class="moss-cards" data-layout="list">...</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "1",
        description: "Retired: the additional co-class on `.moss-cards[data-layout=\"list\"]` had no matching rules in the default CSS once `children_style: summary` collapsed into the list-layout block, and its lingering emission broke themes that hid the class (e.g. SoCiviC's `.moss/theme/style.css` keyed `display: none` on it, erasing folder-embed listings). Theme authors targeting summary listings should use `.moss-cards[data-layout=\"list\"]` directly.",
    },
    // -------------------------------------------------------------------
    // Cards family — current emitted vocabulary (pre-Phase 1c collapsing).
    // Three parallel layouts: grid, list, minimal. Each has its own
    // container + instance + sub-classes.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-cards-grid",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-cards-grid">
  <a class="moss-card-grid" href="...">...</a>
</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-cards[data-layout=grid]`.",
    },
    ComponentEntry {
        class: "moss-cards-list",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-cards-list">
  <a class="moss-card-list" href="...">...</a>
</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-cards[data-layout=list]`.",
    },
    ComponentEntry {
        class: "moss-cards-minimal-year-group",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<section class="moss-cards-minimal-year-group">
  <h3>2024</h3>
  <div class="moss-card-minimal">...</div>
</section>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Year-grouped section in minimal card layout (e.g. blog index). Modifier `--summary` collapses past years.",
    },
    ComponentEntry {
        class: "moss-cards-minimal-year-group--summary",
        kind: "container",
        parent: "moss-cards-minimal-year-group",
        data_attrs: &[],
        example_html: r#"<section class="moss-cards-minimal-year-group moss-cards-minimal-year-group--summary">...</section>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "BEM modifier on `.moss-cards-minimal-year-group`. Applied to year groups that should render in collapsed summary form (e.g. past years on a blog index).",
    },
    ComponentEntry {
        class: "moss-card",
        kind: "instance",
        parent: "moss-cards",
        data_attrs: &[
            DataAttr {
                name: "data-linkblog",
                values: &[],
                default: "",
                description: "Presence flag: emitted IFF the card's source page has an `external_url:` frontmatter (linkblog pattern). When set, the element is a `<div>` rather than `<a>` so the kicker can host a nested `<a>★</a>` archive link; title, cover, and description carry their own inner anchors to the canonical URL. Absent on ordinary cards (single whole-card `<a>`).",
            },
        ],
        example_html: r#"<a class="moss-card" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "v1 collapsed shape — single canonical instance class inside `.moss-cards`. Layout-specific styling targets `.moss-cards[data-layout=X] .moss-card`. Tag is `<a>` for ordinary cards and `<div>` for linkblog cards (`[data-linkblog]`).",
    },
    ComponentEntry {
        class: "moss-card-cover",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-cover"><img src="..." /></div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Cover media slot inside `.moss-card`. Gets `.moss-card-no-cover` modifier when no image is present.",
    },
    ComponentEntry {
        class: "moss-card-no-cover",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-cover moss-card-no-cover"></div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Modifier applied to `.moss-card-cover` when no cover media is available.",
    },
    ComponentEntry {
        class: "moss-card-content",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-content">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Text content slot inside a grid-layout `.moss-card` (kicker + title + meta).",
    },
    ComponentEntry {
        class: "moss-card-row",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-row">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Row wrapper inside a list-layout `.moss-card` holding body + cover side-by-side.",
    },
    ComponentEntry {
        class: "moss-card-body",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-body">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Text body slot of a list-layout `.moss-card`.",
    },
    ComponentEntry {
        class: "moss-card-head",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-head">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Header row of a `.moss-card-body` (title + kicker + meta).",
    },
    ComponentEntry {
        class: "moss-card-title",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<h3 class="moss-card-title">Page title</h3>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Title inside `.moss-card`.",
    },
    ComponentEntry {
        class: "moss-card-meta",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-meta">2024-01-15</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Type-aware metadata slot (date for articles, count for folders, domain for links). Renders ABOVE the title in horizontal mode — filling the kicker position when the explicit `kicker` slot is unset, per `docs/design-system/preview-cards.md:22-30`. To the right of the title in vertical CJK mode (the horizontal kicker position transposed). Meta IS the visual kicker, with the same uppercase overline treatment.",
    },
    ComponentEntry {
        class: "moss-card-kicker",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<span class="moss-card-kicker">Category</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Eyebrow / overline above the title inside `.moss-card`.",
    },
    ComponentEntry {
        class: "moss-card-permalink",
        kind: "instance",
        parent: "moss-card-kicker",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-permalink" href="/posts/foo/" title="Permalink to 'Title'">★</a>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Author's-archive link mark (★, U+2605) emitted INSIDE `.moss-card-kicker` for linkblog cards (those whose child page has `external_url:`). The card title links to the external canonical (publisher); the `★` links to the local archival copy at the page's slug. Reads as part of the kicker line — \"Publisher · Year ★\". Semantically distinct from Daring-Fireball's linkblog ★ (which marks discussion permalink alongside commentary) — here the local copy is the same content preserved for resilience and stable bylines, not added commentary. Putting `<a>★</a>` inside the kicker is valid because linkblog cards emit `<div class=\"moss-card\" data-linkblog>` (not `<a>`) as the outer element — see the `data-linkblog` attribute described on `.moss-card`.",
    },
    ComponentEntry {
        class: "moss-card-title-link",
        kind: "instance",
        parent: "moss-card-head",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-title-link" href="https://outlet.example/article"><h3 class="moss-card-title">Article Title</h3></a>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Anchor wrapping the `.moss-card-title` `<h3>` on linkblog cards. Ordinary cards have the whole-card `<a class=\"moss-card\">` as the link target — but linkblog cards switch the outer to `<div>` so the kicker can host a nested `★` anchor, which means the title needs its own anchor to stay clickable. Same canonical-URL target as the other inner anchors (`moss-card-cover-link`, `moss-card-description-link`).",
    },
    ComponentEntry {
        class: "moss-card-cover-link",
        kind: "instance",
        parent: "moss-card-row",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-cover-link" href="https://outlet.example/article"><div class="moss-card-cover">...</div></a>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Anchor wrapping the `.moss-card-cover` on linkblog cards — same role as `.moss-card-title-link` but for the cover image / media. Targets the canonical (external) URL.",
    },
    ComponentEntry {
        class: "moss-card-description-link",
        kind: "instance",
        parent: "moss-card-body",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-description-link" href="https://outlet.example/article"><p class="moss-card-description">…</p></a>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Anchor wrapping the `.moss-card-description` on linkblog cards — same role as `.moss-card-title-link` but for the description excerpt. Targets the canonical (external) URL.",
    },
    ComponentEntry {
        class: "moss-card-description",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<p class="moss-card-description">Excerpt...</p>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Excerpt / description paragraph inside a `.moss-card` — below the title in both grid- and list-layout cards.",
    },
    ComponentEntry {
        class: "moss-card-count",
        kind: "instance",
        parent: "moss-card",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-count">4 articles</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Tertiary subtitle line showing `N articles` for a folder card. Renders only on non-date listings when the folder card has no `description` to display.",
    },
    ComponentEntry {
        class: "moss-embed-more",
        kind: "instance",
        parent: "moss-cards-container",
        data_attrs: &[],
        example_html: r#"<p class="moss-embed-more"><a href="/news/">More →</a></p>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Trailing \"More →\" link on a truncated children listing (emitted when `children_limit` caps the embed); links to the folder's full index. Rendered as a sibling immediately after `.moss-cards-container`, so it sits outside the listing's flex `gap` and binds to the list via its own `margin-top` (see docs/architecture/ui-design/spacing.md).",
    },
    ComponentEntry {
        class: "moss-card-grid",
        kind: "instance",
        parent: "moss-cards-grid",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-grid" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card` (with parent `.moss-cards[data-layout=grid]`).",
    },
    ComponentEntry {
        class: "moss-card-grid-cover",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-grid-cover"><img src="..." /></div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-cover`.",
    },
    ComponentEntry {
        class: "moss-card-grid-no-cover",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-grid-cover moss-card-grid-no-cover"></div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-no-cover`.",
    },
    ComponentEntry {
        class: "moss-card-grid-content",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-grid-content">...</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-content`.",
    },
    ComponentEntry {
        class: "moss-card-grid-kicker",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<span class="moss-card-grid-kicker">Category</span>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-kicker`.",
    },
    ComponentEntry {
        class: "moss-card-grid-title",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<h3 class="moss-card-grid-title">Page title</h3>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-title`.",
    },
    ComponentEntry {
        class: "moss-card-grid-meta",
        kind: "instance",
        parent: "moss-card-grid",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-grid-meta">2024-01-15</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-meta`.",
    },
    ComponentEntry {
        class: "moss-card-list",
        kind: "instance",
        parent: "moss-cards-list",
        data_attrs: &[],
        example_html: r#"<a class="moss-card-list" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card` (with parent `.moss-cards[data-layout=list]`).",
    },
    ComponentEntry {
        class: "moss-card-list-row",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-list-row">...</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-row`.",
    },
    ComponentEntry {
        class: "moss-card-list-cover",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-list-cover"><img src="..." /></div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-cover`.",
    },
    ComponentEntry {
        class: "moss-card-list-body",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-list-body">...</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-body`.",
    },
    ComponentEntry {
        class: "moss-card-list-head",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-list-head">...</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-head`.",
    },
    ComponentEntry {
        class: "moss-card-list-kicker",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<span class="moss-card-list-kicker">Category</span>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-kicker`.",
    },
    ComponentEntry {
        class: "moss-card-list-title",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<h3 class="moss-card-list-title">Page title</h3>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-title`.",
    },
    ComponentEntry {
        class: "moss-card-list-meta",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-list-meta">2024-01-15</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-meta`.",
    },
    ComponentEntry {
        class: "moss-card-list-description",
        kind: "instance",
        parent: "moss-card-list",
        data_attrs: &[],
        example_html: r#"<p class="moss-card-list-description">Excerpt...</p>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card-description`.",
    },
    ComponentEntry {
        class: "moss-card-minimal",
        kind: "instance",
        parent: "moss-cards-minimal-year-group",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-minimal">
  <a class="moss-prefix-link" href="...">...</a>
</div>"#,
        example_markdown: "",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed into `.moss-card` (with parent `.moss-cards[data-layout=minimal]`).",
    },
    ComponentEntry {
        class: "moss-folder-item",
        kind: "instance",
        parent: "moss-cards-minimal-year-group",
        data_attrs: &[],
        example_html: r#"<div class="moss-card-minimal moss-folder-item">
  <a class="moss-prefix-link moss-folder-link" href="...">...</a>
  <p class="moss-folder-description">...</p>
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Modifier on `.moss-card-minimal` for folder-type entries in minimal listings.",
    },
    ComponentEntry {
        class: "moss-folder-title",
        kind: "instance",
        parent: "moss-folder-item",
        data_attrs: &[],
        example_html: r#"<span class="moss-folder-title">Folder name</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Title text of a folder entry in minimal listings.",
    },
    ComponentEntry {
        class: "moss-folder-description",
        kind: "instance",
        parent: "moss-folder-item",
        data_attrs: &[],
        example_html: r#"<p class="moss-folder-description">Description...</p>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Description paragraph of a folder entry in minimal listings.",
    },
    ComponentEntry {
        class: "moss-folder-link",
        kind: "instance",
        parent: "moss-folder-item",
        data_attrs: &[],
        example_html: r#"<a class="moss-prefix-link moss-folder-link" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Modifier on `.moss-prefix-link` for folder-type links in minimal listings.",
    },
    // -------------------------------------------------------------------
    // Prefix-link primitive — used by minimal cards and other listings.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-prefix-link",
        kind: "instance",
        parent: "moss-card-minimal",
        data_attrs: &[],
        example_html: r#"<a class="moss-prefix-link" href="...">
  <span class="moss-prefix-link-prefix">2024-01-15</span>
  <span class="moss-prefix-link-title">Page title</span>
</a>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Link with a prefix span (date or icon) and a title span. Used inside minimal cards.",
    },
    ComponentEntry {
        class: "moss-prefix-link-prefix",
        kind: "instance",
        parent: "moss-prefix-link",
        data_attrs: &[],
        example_html: r#"<span class="moss-prefix-link-prefix">2024-01-15</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Prefix slot of a prefix-link (typically a date).",
    },
    ComponentEntry {
        class: "moss-prefix-link-title",
        kind: "instance",
        parent: "moss-prefix-link",
        data_attrs: &[],
        example_html: r#"<span class="moss-prefix-link-title">Page title</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Title slot of a prefix-link.",
    },
    ComponentEntry {
        class: "moss-prefix-link-suffix",
        kind: "instance",
        parent: "moss-prefix-link",
        data_attrs: &[],
        example_html: r#"<span class="moss-prefix-link-suffix">→</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Optional trailing slot of a prefix-link.",
    },
    // -------------------------------------------------------------------
    // Callouts — Obsidian-style admonitions. Type variant goes on the
    // container as `.callout-<type>`. Phase 1c may collapse into
    // `.moss-callout[data-type]`.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-callout",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout" data-type="note">
  <div class="callout-title">Note</div>
  <div class="callout-content">Body...</div>
</div>"#,
        example_markdown: "> [!note]\n> Body...",
        status: Status::Confirmed,
        since: "0",
        description: "Obsidian-style callout. The Obsidian-compat `.callout` class is co-emitted; type lives on `data-type` (v1).",
    },
    ComponentEntry {
        class: "callout",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-type",
                values: &["note", "info", "tip", "warning", "pending"],
                default: "note",
                description: "v1 callout type. Theme authors target `.callout[data-type=...]` to style by variant.",
            },
        ],
        example_html: r#"<div class="moss-callout callout" data-type="note">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Obsidian-compat class co-emitted on every callout for theme parity. Type lives on `data-type` (v1).",
    },
    ComponentEntry {
        class: "callout-title",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="callout-title">Note</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Title row of a callout.",
    },
    ComponentEntry {
        class: "callout-content",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="callout-content">Body...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Body container of a callout.",
    },
    ComponentEntry {
        class: "callout-note",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout callout-note">...</div>"#,
        example_markdown: "> [!note]\n> Body",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — type lives on `.callout[data-type=note]`.",
    },
    ComponentEntry {
        class: "callout-info",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout callout-info">...</div>"#,
        example_markdown: "> [!info]\n> Body",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — type lives on `.callout[data-type=info]`.",
    },
    ComponentEntry {
        class: "callout-tip",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout callout-tip">...</div>"#,
        example_markdown: "> [!tip]\n> Body",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — type lives on `.callout[data-type=tip]`.",
    },
    ComponentEntry {
        class: "callout-warning",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout callout-warning">...</div>"#,
        example_markdown: "> [!warning]\n> Body",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — type lives on `.callout[data-type=warning]`.",
    },
    ComponentEntry {
        class: "callout-pending",
        kind: "instance",
        parent: "moss-callout",
        data_attrs: &[],
        example_html: r#"<div class="moss-callout callout callout-pending">...</div>"#,
        example_markdown: "> [!pending]\n> Body",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — type lives on `.callout[data-type=pending]`.",
    },
    // -------------------------------------------------------------------
    // Embeds — `![[file.ext]]` shortcode renderers (audio, video, pdf,
    // notebook, table, 3d, iframe).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-embed",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-type",
                values: &["audio", "video", "pdf", "notebook", "table", "iframe", "3d"],
                default: "",
                description: "v1 embed kind. Set on the embed element. Theme authors target `.moss-embed[data-type=...]`.",
            },
            DataAttr {
                name: "data-loop",
                values: &[],
                default: "",
                description: "Ambient background video: autoplay + muted + loop + playsinline, controls off. Authored as `![[clip.mp4|loop]]`. Boolean presence flag (value is empty). JS reads it to apply the reduced-motion guard and mount the pause/play toggle.",
            },
            DataAttr {
                name: "data-width",
                values: &["body", "wide", "page", "screen"],
                default: "body",
                description: "Display width — text-column (body), wider than text (wide), page-width (page), or viewport-width (screen). See spec § P9.",
            },
            DataAttr {
                name: "data-provider",
                values: &["youtube", "vimeo", "codepen"],
                default: "",
                description: "Identifies the embed provider for external URL embeds. Absent for generic iframes and local HTML embeds.",
            },
        ],
        example_html: r#"<video class="moss-embed moss-embed-video" data-type="video" data-loop src="clip.mp4" autoplay muted loop playsinline preload="metadata"></video>"#,
        example_markdown: "![[clip.mp4|loop]]",
        status: Status::Confirmed,
        since: "0",
        description: "Base class on every typed embed. Kind on `data-type` (v1). Ambient video: add `data-loop` via `![[clip.mp4|loop]]`. `.moss-embed-audio` / `-video` / `-pdf` / `-notebook` / `-table` / `-iframe` / `-3d` retired in Phase 1c.",
    },
    ComponentEntry {
        class: "moss-embed-audio",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-audio"><audio controls src="..."></audio></div>"#,
        example_markdown: "![[track.mp3]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=audio]`.",
    },
    ComponentEntry {
        class: "moss-embed-video",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-video"><video controls src="..."></video></div>"#,
        example_markdown: "![[clip.mp4]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=video]`.",
    },
    ComponentEntry {
        class: "moss-embed-pdf",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-pdf"><iframe src="..."></iframe></div>"#,
        example_markdown: "![[paper.pdf]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=pdf]`.",
    },
    ComponentEntry {
        class: "moss-embed-iframe",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-iframe"><iframe src="..."></iframe></div>"#,
        example_markdown: "![[page.html]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=iframe]`.",
    },
    ComponentEntry {
        class: "moss-embed-notebook",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-notebook">...</div>"#,
        example_markdown: "![[analysis.ipynb]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=notebook]`.",
    },
    ComponentEntry {
        class: "moss-embed-ipynb",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-ipynb">...</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Alias of `.moss-embed-notebook`; consolidation pending.",
    },
    ComponentEntry {
        class: "moss-embed-table",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-table"><table>...</table></div>"#,
        example_markdown: "![[data.csv]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=table]`.",
    },
    ComponentEntry {
        class: "moss-embed-3d",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-3d">...</div>"#,
        example_markdown: "![[model.glb]]",
        status: Status::Retired,
        since: "0",
        description: "Retired in Phase 1c — collapsed to `.moss-embed[data-type=3d]`.",
    },
    ComponentEntry {
        class: "moss-embed-error",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed moss-embed-error">File not found: ...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Error state for embeds whose target cannot be resolved.",
    },
    ComponentEntry {
        class: "moss-embed-missing",
        kind: "instance",
        parent: "moss-embed",
        data_attrs: &[],
        example_html: r#"<div class="moss-embed-missing">Folder not found: journal</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "1",
        description: "Fallback rendered when a folder-list embed (`![[journal/]]`) targets a folder that does not exist or cannot be resolved. Distinct from `.moss-embed-error` (file/wikilink resolution failure) — this one is specifically the folder-listing path.",
    },
    // -------------------------------------------------------------------
    // Hero, image, visual primitives.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-hero",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-width",
                values: &["body", "wide", "page", "screen"],
                default: "body",
                description: "Display width — text-column (body), wider than text (wide), page-width (page), or viewport-width (screen). See spec § P9. Phase 1c will emit this from authoring shortcode (e.g. `:::hero {full}` -> `data-width=\"screen\"`).",
            },
        ],
        example_html: r#"<section class="moss-hero" data-width="page">
  <div class="moss-hero-content">...</div>
</section>"#,
        example_markdown: ":::hero {image=cover.jpg}\n:::\n",
        status: Status::Confirmed,
        since: "0",
        description: "Hero banner section at the top of a page (cover image + title). v1 adds `data-width` for author-controlled sizing.",
    },
    ComponentEntry {
        class: "moss-hero-content",
        kind: "instance",
        parent: "moss-hero",
        data_attrs: &[],
        example_html: r#"<div class="moss-hero-content">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Text content slot inside `.moss-hero`.",
    },
    ComponentEntry {
        class: "moss-image",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-aspect",
                values: &["portrait", "square", "auto"],
                default: "auto",
                description: "v1 image aspect-ratio hint. Theme authors target `.moss-image[data-aspect=...]`. Emitter wiring lands in a follow-up.",
            },
            DataAttr {
                name: "data-width",
                values: &["body", "wide", "page", "screen"],
                default: "body",
                description: "Display width — text-column (body), wider than text (wide), page-width (page), or viewport-width (screen). See spec § P9.",
            },
        ],
        example_html: r#"<figure class="moss-image" style="width:55%"><img src="..." alt="..." /></figure>"#,
        example_markdown: "![alt](image.jpg)",
        status: Status::Confirmed,
        since: "0",
        description: "Wrapper around an inline `<img>` for sizing and figure semantics. `data-width` carries a named width token (body|wide|page|screen); a content-relative width is instead emitted as inline `style=\"width:NN%\"` (set by the editor drag-resize), which also forces the inner image to fill that percent box. Images narrower than the content column center horizontally.",
    },
    ComponentEntry {
        class: "moss-align-left",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<img src="..." alt="..." class="moss-align-left" />"#,
        example_markdown: "![[photo.jpg|align-left]]",
        status: Status::Confirmed,
        since: "0",
        description: "Floats an image to the left of body text (editorial runaround). Defaults max-width to 50% on desktop, collapses to full-width below 48rem. CSS `:has()` escalates the float to a wrapping `<figure class=\"moss-image\">` or `<picture>` when present. Mirrors WordPress's `alignleft` convention.",
    },
    ComponentEntry {
        class: "moss-align-right",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<img src="..." alt="..." class="moss-align-right" />"#,
        example_markdown: "![[photo.jpg|align-right]]",
        status: Status::Confirmed,
        since: "0",
        description: "Floats an image to the right of body text (editorial runaround). Symmetric counterpart to `.moss-align-left`. Mirrors WordPress's `alignright` convention.",
    },
    ComponentEntry {
        class: "moss-article-title",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<h1 class="moss-article-title">Title</h1>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Article-page H1 title emitted from frontmatter.",
    },
    ComponentEntry {
        class: "moss-heading-anchor",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r##"<h2 id="setup">Setup<a class="moss-heading-anchor" href="#setup" aria-label="Permalink to this section"><span aria-hidden="true">#</span></a></h2>"##,
        example_markdown: "## Setup",
        status: Status::Emerging,
        since: "1",
        description: "Clickable permalink appended inside every author-written body heading that carries a slug id; links to the heading's `#`-fragment. The auto-injected `moss-article-title` H1 is emitted separately and gets no anchor.",
    },
    // -------------------------------------------------------------------
    // Grid + gallery + buttons containers (free-form layouts).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-grid",
        kind: "container",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-width",
                values: &["body", "wide", "page", "screen"],
                default: "body",
                description: "Display width — text-column (body), wider than text (wide), page-width (page), or viewport-width (screen). See spec § P9.",
            },
        ],
        example_html: r#"<div class="moss-grid" data-width="wide">
  <div class="moss-grid-card">...</div>
</div>"#,
        example_markdown: ":::grid {cols=2}\nLeft cell\n+++\nRight cell\n:::\n",
        status: Status::Confirmed,
        since: "0",
        description: "Generic grid container (used by profiles, link previews, etc.). Modifier classes: `profiles`, `featured`, `no-cards`. v1 adds `data-width` (P9).",
    },
    ComponentEntry {
        class: "moss-grid-card",
        kind: "instance",
        parent: "moss-grid",
        data_attrs: &[
            DataAttr {
                name: "data-kind",
                values: &["link", "friend", "card"],
                default: "card",
                description: "v1 grid-card variant. Today expressed via co-emitted classes (`.link-card`, `.friend-card`, `.no-cards`); Phase 1c collapses to this `data-kind` attribute.",
            },
        ],
        example_html: r#"<a class="moss-grid-card" data-kind="link" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Card instance inside `.moss-grid`. Today emits sibling classes `link-card` / `friend-card` / `no-cards`; v1 collapses to `data-kind`.",
    },
    ComponentEntry {
        class: "moss-gallery",
        kind: "container",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-width",
                values: &["body", "wide", "page", "screen"],
                default: "body",
                description: "Display width — text-column (body), wider than text (wide), page-width (page), or viewport-width (screen). See spec § P9.",
            },
        ],
        example_html: r#"<div class="moss-gallery" data-width="page">
  <figure class="moss-gallery-item">...</figure>
</div>"#,
        example_markdown: ":::gallery\nphoto.jpg\n:::\n",
        status: Status::Confirmed,
        since: "0",
        description: "Image gallery container. v1 adds `data-width` (P9).",
    },
    ComponentEntry {
        class: "moss-gallery-item",
        kind: "instance",
        parent: "moss-gallery",
        data_attrs: &[],
        example_html: r#"<figure class="moss-gallery-item"><img src="..." /></figure>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Single image entry inside `.moss-gallery`.",
    },
    ComponentEntry {
        class: "moss-buttons",
        kind: "container",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-style",
                values: &["default", "inverted"],
                default: "default",
                description: "v1 button-row style. Theme authors target `.moss-buttons[data-style=...]`.",
            },
        ],
        example_html: r#"<div class="moss-buttons" data-style="inverted">
  <a class="moss-btn" href="...">Click</a>
</div>"#,
        example_markdown: ":::buttons\n[Get started](https://example.com)\n:::\n",
        status: Status::Confirmed,
        since: "0",
        description: "Container for a row of `.moss-btn` buttons. v1: the inverted variant is on `data-style=\"inverted\"`.",
    },
    // -------------------------------------------------------------------
    // Button primitive (used by subscribe + general CTAs).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-btn",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-role",
                values: &["default", "primary", "secondary"],
                default: "default",
                description: "v1 button role. Theme authors target `.moss-btn[data-role=...]`.",
            },
        ],
        example_html: r#"<button class="moss-btn" data-role="primary">
  <span class="moss-btn__label">Submit</span>
</button>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Generic button primitive. Role on `data-role` (v1).",
    },
    ComponentEntry {
        class: "moss-btn__label",
        kind: "instance",
        parent: "moss-btn",
        data_attrs: &[],
        example_html: r#"<span class="moss-btn__label">Submit</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Label span inside `.moss-btn`.",
    },
    ComponentEntry {
        class: "moss-btn__check",
        kind: "instance",
        parent: "moss-btn",
        data_attrs: &[],
        example_html: r#"<span class="moss-btn__check">✓</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Success checkmark slot inside `.moss-btn`.",
    },
    ComponentEntry {
        class: "moss-btn__spinner",
        kind: "instance",
        parent: "moss-btn",
        data_attrs: &[],
        example_html: r#"<span class="moss-btn__spinner"></span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Loading spinner slot inside `.moss-btn`.",
    },
    // -------------------------------------------------------------------
    // Subscribe form (newsletter / Buttondown / seta).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-subscribe",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-subscribe">
  <form class="moss-subscribe-form">...</form>
</div>"#,
        example_markdown: ":::subscribe\n:::\n",
        status: Status::Confirmed,
        since: "0",
        description: "Newsletter subscribe block (auto-injected into footer when email channel configured).",
    },
    ComponentEntry {
        class: "moss-subscribe-form",
        kind: "instance",
        parent: "moss-subscribe",
        data_attrs: &[
            DataAttr {
                name: "data-position",
                values: &["inline", "apply"],
                default: "inline",
                description: "Placement/behavior variant. All moss-hosted subscribe forms are `inline` (the auto-injected footer form and the `:::subscribe` shortcode emit identical HTML — footer vs in-page styling keys on the `footer` ancestor in CSS, not this attribute). `apply` marks the `:::apply` form (terminal success, FormData body).",
            },
            DataAttr {
                name: "data-button-override",
                values: &["true"],
                default: "true",
                description: "Emitted only when the author overrides the button label (`:::subscribe{button=\"...\"}`). Signals subscribe.ts to leave the button label AND placeholder as authored instead of overwriting them with the language-default copy.",
            },
            DataAttr {
                name: "data-moss-hosted",
                values: &["true"],
                default: "true",
                description: "Marks moss-hosted (seta) forms hydrated by subscribe.ts. Absent on 3rd-party provider forms.",
            },
            DataAttr {
                name: "data-state",
                values: &["idle", "loading", "success", "error"],
                default: "idle",
                description: "Runtime submit state machine, driven by subscribe.ts. Emitted as `idle`; theme authors target `.moss-subscribe-form[data-state=...]`.",
            },
            DataAttr {
                name: "data-moss-pending-site",
                values: &["true"],
                default: "true",
                description: "Pre-first-publish pending wiring (`action=\"#\"`, no site_id yet). Hidden on published pages (body without `data-moss-preview`) via the email.css defense rule so a pending form never faces real readers.",
            },
        ],
        example_html: r#"<form class="moss-subscribe-form">...</form>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Form element inside `.moss-subscribe`.",
    },
    ComponentEntry {
        class: "moss-btn-slot",
        kind: "instance",
        parent: "moss-subscribe",
        data_attrs: &[],
        example_html: r#"<div class="moss-btn-slot"><button class="moss-btn">...</button></div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Fixed-width slot wrapping a form's submit button; used by both the subscribe and comment forms to prevent layout shift across idle/loading/success states.",
    },
    ComponentEntry {
        class: "moss-subscribe-status",
        kind: "instance",
        parent: "moss-subscribe",
        data_attrs: &[],
        example_html: r#"<div class="moss-subscribe-status">
  <span class="moss-subscribe-status__icon"></span>
  Subscribed!
</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Status message shown after submit (success/error).",
    },
    ComponentEntry {
        class: "moss-subscribe-status__icon",
        kind: "instance",
        parent: "moss-subscribe-status",
        data_attrs: &[],
        example_html: r#"<span class="moss-subscribe-status__icon"></span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Icon slot inside `.moss-subscribe-status`.",
    },
    ComponentEntry {
        class: "moss-subscribe-landing",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<section class="moss-subscribe-landing">...</section>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Standalone subscribe landing page surface (larger variant).",
    },
    // -------------------------------------------------------------------
    // Apply form (membership / contributor application).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-apply",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-apply" data-state="idle">
  <form class="moss-subscribe-form moss-apply-form">...</form>
</div>"#,
        example_markdown: ":::apply\n:::\n",
        status: Status::Emerging,
        since: "0",
        description: "Apply / membership-request form block (:::apply shortcode).",
    },
    ComponentEntry {
        class: "moss-apply-form",
        kind: "instance",
        parent: "moss-apply",
        data_attrs: &[
            DataAttr {
                name: "data-position",
                values: &["apply"],
                default: "apply",
                description: "Position variant; always `apply` for this form. Drives CSS layout in email.css.",
            },
            DataAttr {
                name: "data-revert",
                values: &["false"],
                default: "false",
                description: "When `false`, success is terminal (no auto-revert). subscribe.ts reads this.",
            },
        ],
        example_html: r#"<form class="moss-subscribe-form moss-apply-form" data-position="apply" data-revert="false">...</form>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Form element inside `.moss-apply`. Also carries `.moss-subscribe-form` so subscribe.ts hydrates it.",
    },
    ComponentEntry {
        class: "moss-apply-matters",
        kind: "instance",
        parent: "moss-apply",
        data_attrs: &[],
        example_html: r#"<input type="text" name="matters" class="moss-input moss-apply-matters">"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Second apply-form input inside `.moss-apply-form` — a Matters username OR a one-line pitch (placeholder-only, no visible label).",
    },
    ComponentEntry {
        class: "moss-apply-hp",
        kind: "instance",
        parent: "moss-apply",
        data_attrs: &[],
        example_html: r#"<input type="text" name="website" class="moss-apply-hp" tabindex="-1" aria-hidden="true">"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Honeypot field (off-screen) inside `.moss-apply-form`. Bots fill it; humans don't.",
    },
    ComponentEntry {
        class: "moss-apply-status",
        kind: "instance",
        parent: "moss-apply",
        data_attrs: &[],
        example_html: r#"<div class="moss-subscribe-status moss-apply-status" aria-live="polite">...</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Status region inside `.moss-apply-form` (also carries `.moss-subscribe-status`).",
    },
    ComponentEntry {
        class: "moss-apply-helper",
        kind: "instance",
        parent: "moss-apply",
        data_attrs: &[],
        example_html: r#"<p class="moss-apply-helper" id="moss-apply-email-help">用于获取邀请及免费托管服务</p>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Helper text line beneath each field in `.moss-apply-form` (referenced by the field's aria-describedby). Internal — not part of the public component contract.",
    },
    // -------------------------------------------------------------------
    // Series navigation (prev/next + collection links).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-series-nav",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<nav class="moss-series-nav">
  <div class="moss-series-nav-links">...</div>
</nav>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Series navigation bar (prev/next/collection) on series pages.",
    },
    ComponentEntry {
        class: "moss-series-nav-links",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<div class="moss-series-nav-links">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Row holding prev/next links in series nav.",
    },
    ComponentEntry {
        class: "moss-series-nav-link",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<a class="moss-series-nav-link moss-series-nav-prev" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Individual link inside series nav. Modifiers: `moss-series-nav-prev`, `moss-series-nav-next`, `empty` (placeholder).",
    },
    ComponentEntry {
        class: "moss-series-nav-prev",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<a class="moss-series-nav-link moss-series-nav-prev" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Previous-page modifier on a series nav link.",
    },
    ComponentEntry {
        class: "moss-series-nav-next",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<a class="moss-series-nav-link moss-series-nav-next" href="...">...</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Next-page modifier on a series nav link.",
    },
    ComponentEntry {
        class: "moss-series-nav-arrow",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<span class="moss-series-nav-arrow">→</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Arrow glyph inside a series-nav link.",
    },
    ComponentEntry {
        class: "moss-series-nav-title",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<span class="moss-series-nav-title">Next page title</span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Title text of the destination page in a series-nav link.",
    },
    ComponentEntry {
        class: "moss-series-nav-collection",
        kind: "instance",
        parent: "moss-series-nav",
        data_attrs: &[],
        example_html: r#"<div class="moss-series-nav-collection">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Collection-listing slot in series nav (sibling pages).",
    },
    ComponentEntry {
        class: "moss-series-nav-collection-row",
        kind: "instance",
        parent: "moss-series-nav-collection",
        data_attrs: &[],
        example_html: r#"<div class="moss-series-nav-collection-row">...</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Row inside the collection listing of series nav.",
    },
    // -------------------------------------------------------------------
    // Collection cover (collection landing pages).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-collection-cover",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<section class="moss-collection-cover">
  <div class="moss-collection-cover-row">...</div>
</section>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Header surface on a collection landing page.",
    },
    ComponentEntry {
        class: "moss-collection-cover-row",
        kind: "instance",
        parent: "moss-collection-cover",
        data_attrs: &[],
        example_html: r#"<div class="moss-collection-cover-row">...</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Row inside `.moss-collection-cover`.",
    },
    ComponentEntry {
        class: "moss-collection-cover-body",
        kind: "instance",
        parent: "moss-collection-cover",
        data_attrs: &[],
        example_html: r#"<div class="moss-collection-cover-body">...</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Body content slot inside `.moss-collection-cover`.",
    },
    // -------------------------------------------------------------------
    // Form primitives (input, label, field, link).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-input",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<input class="moss-input" type="email" />"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Generic form input primitive.",
    },
    ComponentEntry {
        class: "moss-field",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-field">
  <label class="moss-label">Email</label>
  <input class="moss-input" />
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Form field group (label + input). Modifier `--inline` for horizontal layout.",
    },
    ComponentEntry {
        class: "moss-label",
        kind: "instance",
        parent: "moss-field",
        data_attrs: &[],
        example_html: r#"<label class="moss-label">Email</label>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Label primitive for `.moss-field`. Modifier `--small` for compact form.",
    },
    ComponentEntry {
        class: "moss-link",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<a class="moss-link" href="...">Click me</a>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Inline-link primitive (resets `<button>` chrome too). Use `--subtle` for muted variant.",
    },
    ComponentEntry {
        class: "moss-field--inline",
        kind: "instance",
        parent: "moss-field",
        data_attrs: &[],
        example_html: r#"<div class="moss-field moss-field--inline">
  <label class="moss-label">Email</label>
  <input class="moss-input" />
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "BEM modifier on `.moss-field` for horizontal label+input layout (used by settings UI primitives).",
    },
    ComponentEntry {
        class: "moss-label--small",
        kind: "instance",
        parent: "moss-label",
        data_attrs: &[],
        example_html: r#"<label class="moss-label moss-label--small">Compact label</label>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "BEM modifier on `.moss-label` for compact form (used by services settings rows).",
    },
    ComponentEntry {
        class: "moss-info-grid",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-info-grid">
  <div class="moss-field moss-field--inline">...</div>
  <div class="moss-field moss-field--inline">...</div>
</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Two-column aligned label+value rows (CSS grid with `display: contents` children). Used by the deployment settings panel; ships in the default theme so authors can reuse the layout.",
    },
    ComponentEntry {
        class: "moss-row",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-row">
  <div class="moss-field">...</div>
  <div class="moss-field">...</div>
</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Horizontal flex row of equal-flex `.moss-field` children. Form-row layout helper shipped in the default theme.",
    },
    ComponentEntry {
        class: "moss-input-feedback",
        kind: "instance",
        parent: "moss-field",
        data_attrs: &[],
        example_html: r#"<span class="moss-input-feedback">Saving…</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Auto-save status hint slot under `.moss-field`. Three state modifiers: `--success`, `--error`, `--fade-out`.",
    },
    ComponentEntry {
        class: "moss-input-feedback--success",
        kind: "instance",
        parent: "moss-input-feedback",
        data_attrs: &[],
        example_html: r#"<span class="moss-input-feedback moss-input-feedback--success">Saved</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Success state modifier on `.moss-input-feedback`.",
    },
    ComponentEntry {
        class: "moss-input-feedback--error",
        kind: "instance",
        parent: "moss-input-feedback",
        data_attrs: &[],
        example_html: r#"<span class="moss-input-feedback moss-input-feedback--error">Failed to save</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Error state modifier on `.moss-input-feedback`.",
    },
    ComponentEntry {
        class: "moss-input-feedback--fade-out",
        kind: "instance",
        parent: "moss-input-feedback",
        data_attrs: &[],
        example_html: r#"<span class="moss-input-feedback moss-input-feedback--success moss-input-feedback--fade-out">Saved</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Transient fade-out modifier on `.moss-input-feedback` (applied after a success message to dismiss it).",
    },
    // -------------------------------------------------------------------
    // Other emit surfaces (comments, colophon, shell frame, misc).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-comments",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<section class="moss-comments">...</section>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Comments surface (per-site SQLite backend or Artalk legacy).",
    },
    ComponentEntry {
        class: "moss-service-inactive",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<section class="moss-comments moss-service-inactive">...</section>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Co-class applied to `.moss-comments` and `.moss-subscribe-form` when the backing service is not configured. Hidden by default in published sites and revealed inside the preview chrome so authors can see the inactive surface during editing.",
    },
    // -------------------------------------------------------------------
    // Preview link popover — emitted by `assets/js/preview.js` runtime.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-preview-popup",
        kind: "chrome",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-preview-popup" role="tooltip" aria-live="polite">
  <strong class="moss-preview-title">...</strong>
  <p class="moss-preview-desc">...</p>
  <p class="moss-preview-text">...</p>
</div>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Floating link-preview popover injected at `document.body` level by the runtime `preview.js`. Fetches `/_moss/previews.json` and renders a hover card with title, description, and excerpt for internal links.",
    },
    ComponentEntry {
        class: "moss-preview-title",
        kind: "instance",
        parent: "moss-preview-popup",
        data_attrs: &[],
        example_html: r#"<strong class="moss-preview-title">Article title</strong>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Title slot inside `.moss-preview-popup`.",
    },
    ComponentEntry {
        class: "moss-preview-desc",
        kind: "instance",
        parent: "moss-preview-popup",
        data_attrs: &[],
        example_html: r#"<p class="moss-preview-desc">Short description</p>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Description slot inside `.moss-preview-popup` (from frontmatter `description`).",
    },
    ComponentEntry {
        class: "moss-preview-text",
        kind: "instance",
        parent: "moss-preview-popup",
        data_attrs: &[],
        example_html: r#"<p class="moss-preview-text">Excerpt of the linked article…</p>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Excerpt slot inside `.moss-preview-popup` (auto-extracted from the linked article body).",
    },
    // -------------------------------------------------------------------
    // Missing-image fallback marker — added by the inline
    // `#moss-img-fallback` script in shell.html at runtime, when an <img>
    // fails to load.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-img-fallback",
        kind: "chrome",
        parent: "",
        data_attrs: &[],
        example_html: r#"<img class="site-logo moss-img-fallback" src="data:image/svg+xml,..." alt="" aria-hidden="true">"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Marker class a capture-phase `error` listener (inlined in shell.html, not a separate hashed asset) adds to any <img> whose load fails (deleted/renamed/typo'd source — never enters moss's AssetRegistry, so nothing server-side can placeholder it). The browser's native broken-image icon never appears: the script swaps the <img>'s OWN `src` in place to a self-contained blueprint-grid-pattern SVG data URI (same blueprint-blue as the animated frontend/app/components/blueprint-grid.ts canvas, without the per-instance canvas/RAF cost) and strips any enclosing <picture>'s <source> children — it does NOT replace the element, so every context-specific sizing/fit rule (.moss-card-cover > img, .moss-hero img, .site-logo, …) keeps applying because the <img>'s tag, class list, and other attributes are untouched.",
    },
    ComponentEntry {
        class: "moss-colophon",
        kind: "chrome",
        parent: "",
        data_attrs: &[],
        example_html: r#"<footer class="moss-colophon">
  <span class="moss-colophon-icon"></span>
  Built with moss
</footer>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Footer colophon credit appended by moss.",
    },
    ComponentEntry {
        class: "moss-colophon-icon",
        kind: "instance",
        parent: "moss-colophon",
        data_attrs: &[],
        example_html: r#"<span class="moss-colophon-icon"></span>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Icon slot inside `.moss-colophon`.",
    },
    ComponentEntry {
        class: "moss-shell-frame",
        kind: "chrome",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-shell-frame">...</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "App-shell frame surface (preview chrome).",
    },
    ComponentEntry {
        class: "moss-mobile-frame",
        kind: "chrome",
        parent: "moss-shell-frame",
        data_attrs: &[],
        example_html: r#"<html class="moss-shell-frame moss-mobile-frame">...</html>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Runtime marker the preview bridge adds to `<html>` when the shell is in mobile device-preview mode; drops the titlebar-clearance padding (the phone frame sits below the titlebar).",
    },
    ComponentEntry {
        class: "main-nav",
        kind: "chrome",
        parent: "",
        data_attrs: &[],
        example_html: r#"<nav class="main-nav container">...</nav>"#,
        example_markdown: "",
        status: Status::Confirmed,
        since: "0",
        description: "Top site navigation bar. Legacy non-`moss-` prefix kept for theme parity.",
    },
    ComponentEntry {
        class: "moss-child-section-divider",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<hr class="moss-child-section-divider" />"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Divider rule between auto-generated child sections.",
    },
    ComponentEntry {
        class: "moss-unknown-shortcode",
        kind: "standalone",
        parent: "",
        data_attrs: &[],
        example_html: r#"<div class="moss-unknown-shortcode">Unknown shortcode: foo</div>"#,
        example_markdown: "{{< foo >}}",
        status: Status::Confirmed,
        since: "0",
        description: "Fallback emitted when a shortcode tag is not recognised by any plugin.",
    },
    // -------------------------------------------------------------------
    // Syntax highlight tokens (emitted by syntect inside <code>).
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-hl-keyword",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-keyword">if</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: keyword.",
    },
    ComponentEntry {
        class: "moss-hl-string",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-string">"hi"</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: string literal.",
    },
    ComponentEntry {
        class: "moss-hl-comment",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-comment">// note</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: comment.",
    },
    ComponentEntry {
        class: "moss-hl-function",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-function">render</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: function name.",
    },
    ComponentEntry {
        class: "moss-hl-type",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-type">String</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: type name.",
    },
    ComponentEntry {
        class: "moss-hl-number",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-number">42</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: numeric literal.",
    },
    ComponentEntry {
        class: "moss-hl-operator",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-operator">+</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: operator.",
    },
    ComponentEntry {
        class: "moss-hl-builtin",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-builtin">print</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: builtin identifier.",
    },
    ComponentEntry {
        class: "moss-hl-tag",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-tag">div</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: markup tag name.",
    },
    ComponentEntry {
        class: "moss-hl-attr",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-attr">class</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: attribute name.",
    },
    ComponentEntry {
        class: "moss-hl-meta",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-meta">@derive</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight token: meta/annotation.",
    },
    ComponentEntry {
        class: "moss-hl-addition-bg",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-addition-bg">+ added line</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight diff token: added-line background.",
    },
    ComponentEntry {
        class: "moss-hl-deletion",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-deletion">- removed line</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight diff token: removed-line text.",
    },
    ComponentEntry {
        class: "moss-hl-deletion-bg",
        kind: "instance",
        parent: "",
        data_attrs: &[],
        example_html: r#"<span class="moss-hl-deletion-bg">- removed line</span>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Syntax-highlight diff token: removed-line background.",
    },
    ComponentEntry {
        class: "moss-recent",
        kind: "container",
        parent: "",
        data_attrs: &[],
        example_html: r#"<ul class="moss-recent">
  <li><a href="/posts/spring-notes/">Spring notes</a><div class="moss-recent__date">2026-04-12</div><div class="moss-recent__desc">A walk through the garden.</div></li>
</ul>"#,
        example_markdown: ":::recent {count=5 since=\"2026-01-01\"}\n:::\n",
        status: Status::Emerging,
        since: "0",
        description: "Auto-generated list of recent posts emitted by the `:::recent` shortcode. Sorted newest-first; date and description slots are filled per child. No default CSS in the bundled theme — theme authors style it freely.",
    },
    ComponentEntry {
        class: "moss-recent__date",
        kind: "instance",
        parent: "moss-recent",
        data_attrs: &[],
        example_html: r#"<div class="moss-recent__date">2026-04-12</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Per-entry date slot inside `.moss-recent` (BEM child). Format is `YYYY-MM-DD`, derived from frontmatter `date`. Empty string when the post lacks a parseable date.",
    },
    ComponentEntry {
        class: "moss-recent__desc",
        kind: "instance",
        parent: "moss-recent",
        data_attrs: &[],
        example_html: r#"<div class="moss-recent__desc">A walk through the garden.</div>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "0",
        description: "Per-entry description slot inside `.moss-recent` (BEM child). Sourced from frontmatter `description`; empty when unset.",
    },
    // -------------------------------------------------------------------
    // Ambient loop video — JS-injected wrapper + toggle (§3.5).
    // The <video data-loop> synthesizer emits `data-loop` on the <video>;
    // ambient-video.ts wraps it at init time.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-ambient-video",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-paused",
                values: &[],
                default: "",
                description: "Boolean presence flag set by ambient-video.ts when the video is paused (user-initiated or reduced-motion guard). CSS uses `[data-paused]` to keep the toggle visible.",
            },
        ],
        example_html: r#"<div class="moss-ambient-video">
  <video data-loop src="clip.mp4" autoplay muted loop playsinline preload="metadata"></video>
  <button class="moss-ambient-toggle" type="button" aria-label="Pause video">⏸</button>
</div>"#,
        example_markdown: "![[clip.mp4|loop]]",
        status: Status::Emerging,
        since: "1",
        description: "JS-injected wrapper around a `video[data-loop]` element. Provides the positioning context for `.moss-ambient-toggle` and the `[data-paused]` state hook. Not emitted by the Rust synthesizer — ambient-video.ts creates it at init.",
    },
    ComponentEntry {
        class: "moss-ambient-toggle",
        kind: "instance",
        parent: "moss-ambient-video",
        data_attrs: &[],
        example_html: r#"<button class="moss-ambient-toggle" type="button" aria-label="Pause video">⏸</button>"#,
        example_markdown: "",
        status: Status::Emerging,
        since: "1",
        description: "Chrome-free pause/play toggle button for ambient loop videos. Injected by ambient-video.ts. Keyboard-focusable; `aria-label` toggles between \"Pause video\" and \"Play video\". Visible on hover/focus of `.moss-ambient-video` and always visible when `[data-paused]`. Satisfies WCAG 2.2.2 Level A (Pause, Stop, Hide).",
    },
    // -------------------------------------------------------------------
    // LaTeX math (ADR-030). P1 emits the escaped source in a marked
    // `<code>`; P2 replaces the element's *contents* with a typeset
    // `<svg>` while keeping the class and `data-moss-math` stable, so a
    // theme selector written against P1 keeps working across the upgrade.
    // -------------------------------------------------------------------
    ComponentEntry {
        class: "moss-math",
        kind: "standalone",
        parent: "",
        data_attrs: &[
            DataAttr {
                name: "data-moss-math",
                values: &["inline", "display"],
                default: "inline",
                description: "Which delimiter produced the equation: `inline` for `$…$`, `display` for `$$…$$`. Carries the distinction to CSS and to the typesetter so neither has to re-derive it from context — a theme can select on it today to centre display math; moss ships no math stylesheet of its own yet, so both variants currently inherit plain `<code>` styling.",
            },
        ],
        example_html: r#"<code class="moss-math" data-moss-math="inline">E = mc^2</code>"#,
        example_markdown: "Energy $E = mc^2$.",
        status: Status::Emerging,
        since: "1",
        description: "A LaTeX equation. In P1 the element holds the author's own TeX source, HTML-escaped — an honest fallback that never shows a blank where an equation was written. Requires `[site].math` (default on).",
    },
];

/// Implementation classes that are emitted by moss for internal functionality
/// but must not appear in the public theme-facing contract (`moss describe` /
/// `docs/contract/reference.md`). These classes ARE present in `COMPONENTS` for
/// the sync-test to validate their HTML class literals, but `is_public()` hides
/// them from agents, themes, and `reference.md` generation.
const INTERNAL_CLASSES: &[&str] = &[
    "moss-apply",
    "moss-apply-form",
    "moss-apply-matters",
    "moss-apply-hp",
    "moss-apply-status",
    "moss-apply-helper",
];

impl ComponentEntry {
    /// True for entries that belong in the public, agent/theme-facing surface.
    /// v1 rule: not retired AND not an internal implementation class.
    ///
    /// Internal classes (e.g. all `moss-apply*`) stay in COMPONENTS so the
    /// sync-test can validate them, but they must not surface in `moss describe`
    /// or `docs/contract/reference.md` — they are subject to change at any time.
    pub fn is_public(&self) -> bool {
        self.status != Status::Retired && !INTERNAL_CLASSES.contains(&self.class)
    }
}

/// Iterator over class names with `Status::Retired`. Used by the build
/// pipeline's theme lint to warn users about pre-v1 vocabulary.
///
/// Exposed as an iterator over `&'static str` so callers don't need to
/// import the `Status` enum (keeps moss-core's surface narrow).
pub fn retired_class_names() -> impl Iterator<Item = &'static str> {
    COMPONENTS.iter()
        .filter(|e| e.status == Status::Retired)
        .map(|e| e.class)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Orphan-gate: every class in `INTERNAL_CLASSES` must exist as a `class`
    /// in `COMPONENTS`. If a class is renamed in the emitter *and* in
    /// `INTERNAL_CLASSES` but forgotten in `COMPONENTS`, it would silently
    /// re-enter the public contract surface (`is_public()` only hides known
    /// internals). This test prevents that gap.
    #[test]
    fn every_internal_class_has_a_components_entry() {
        let component_classes: std::collections::HashSet<&'static str> =
            COMPONENTS.iter().map(|e| e.class).collect();
        for &internal in INTERNAL_CLASSES {
            assert!(
                component_classes.contains(internal),
                "INTERNAL_CLASSES entry '{}' has no matching entry in COMPONENTS — \
                 add a ComponentEntry for it or remove it from INTERNAL_CLASSES",
                internal
            );
        }
    }
}
