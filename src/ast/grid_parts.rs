//! A `:::grid` serialized into its structural pieces.
//!
//! The flat `Grid` arm of [`RenderHooks::render_shortcode`] is
//! [`GridParts::to_html`] over this value, so there is exactly ONE grid byte
//! shape and the split form can never drift from the flat one.
//!
//! The split form exists because a structural decision about a grid *cell* —
//! "is this cell a link to a collection, and if so what is that collection's
//! title, cover and child count?" — depends on facts no single page can see.
//! Before ADR-034 the host answered it by regex-scraping its own emitted
//! markup; that scraping is what aborted the build when a CJK character sat in
//! front of a `:::grid` (moss#903 bug 1). With the pieces kept separate the
//! host pairs each cell's HTML with the typed [`Block`] cell it came from, so
//! recognition reads exactly what emission read.

use super::hooks::{collapse_tag_adjacent_newlines, escape_attr, RenderHooks};
use super::node::Block;
use super::shortcode::GridShortcode;

/// A `:::grid` block's serialized pieces: the opening `<div class="moss-grid"
/// …>` tag and one HTML string per cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridParts {
    /// Carries the author's classes, the ratio `style=`, `data-width`, and
    /// `data-source-range`.
    pub open_tag: String,
    /// One entry per [`GridShortcode::cells`] entry, in the same order. Each
    /// is already wrapped in its card chrome (`<div class="moss-grid-card">`,
    /// or the bare `<a>` a [`Block::LinkCard`] cell renders for itself).
    pub cells: Vec<String>,
}

impl GridParts {
    /// Re-assemble the flat grid HTML — byte-identical to what the `Grid` arm
    /// of [`RenderHooks::render_shortcode`] emits.
    pub fn to_html(&self) -> String {
        let mut out = String::with_capacity(self.open_tag.len() + 8);
        out.push_str(&self.open_tag);
        out.push_str(&self.cells.join("\n"));
        out.push_str("</div>");
        out
    }

    /// Re-assemble with `cells` replaced by `replacement` (same length and
    /// order). Used by a host that swapped some cells for markup derived from
    /// whole-build state and wants the original wrapper back verbatim.
    pub fn to_html_with_cells(&self, replacement: &[String]) -> String {
        let mut out = String::with_capacity(self.open_tag.len() + 8);
        out.push_str(&self.open_tag);
        out.push_str(&replacement.join("\n"));
        out.push_str("</div>");
        out
    }
}

/// The canonical grid serialization. Called by
/// [`RenderHooks::render_grid_parts`]'s default body; overriding impls that
/// delegate the `Grid` arm elsewhere call this with the hooks they delegate to.
pub fn render_grid_parts<H: RenderHooks + ?Sized>(
    hooks: &H,
    args: &GridShortcode,
    source_line: Option<usize>,
) -> GridParts {
    let mut open_tag = String::new();
    {
        let mut class_attr = String::from("moss-grid");
        if !args.classes.is_empty() {
            class_attr.push(' ');
            class_attr.push_str(&args.classes);
        }
        open_tag.push_str(r#"<div class=""#);
        open_tag.push_str(&escape_attr(&class_attr));
        open_tag.push_str(r#"" data-columns=""#);
        open_tag.push_str(&args.columns.to_string());
        open_tag.push('"');
        if let Some(r) = &args.ratio {
            let cols = r
                .split(':')
                .map(|n| format!("{}fr", n.trim()))
                .collect::<Vec<_>>()
                .join(" ");
            open_tag.push_str(r#" style="grid-template-columns:"#);
            open_tag.push_str(&cols);
            open_tag.push('"');
        }
        if let Some(w) = &args.width {
            open_tag.push_str(r#" data-width=""#);
            open_tag.push_str(w);
            open_tag.push('"');
        }
        // Click-to-source: a point source range on the Grid's outermost
        // element routes a click back to the `:::grid` line via the bridge's
        // `resolveSourceTarget`.
        if let Some(n) = source_line {
            open_tag.push_str(r#" data-source-range=""#);
            open_tag.push_str(&n.to_string());
            open_tag.push('-');
            open_tag.push_str(&n.to_string());
            open_tag.push('"');
        }
        open_tag.push('>');
    }

    // Phase 4 PR4.5 (2026-05-28): cells are typed `Vec<Block>`. Each cell
    // renders into its own scratch buffer, gets tag-adjacent newlines
    // collapsed (so the byte shape matches pulldown-cmark's `push_html`), then
    // gets wrapped — or not, for `Block::LinkCard`, which renders its own
    // wrapping `<a>`. `to_html` joins with `\n` to match the byte shape of the
    // long-deleted `render_grid_html_typed`.
    let mut cells: Vec<String> = Vec::with_capacity(args.cells.len());
    // Scope image `sizes=` to the cell track while cells render.
    hooks.begin_grid_cells(args.columns, args.width.as_deref());
    for cell_blocks in &args.cells {
        let mut cell_html = String::new();
        super::render::render_blocks(hooks, &mut cell_html, cell_blocks);
        let collapsed = collapse_tag_adjacent_newlines(&cell_html);
        let trimmed = collapsed.trim();
        cells.push(if let [Block::LinkCard { .. }] = cell_blocks.as_slice() {
            trimmed.to_string()
        } else {
            format!(r#"<div class="moss-grid-card">{}</div>"#, trimmed)
        });
    }
    hooks.end_grid_cells();

    GridParts { open_tag, cells }
}
