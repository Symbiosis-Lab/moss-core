//! Typed frontmatter structs for the build pipeline.
//!
//! Lives in moss-core so validation, the resolver, and src-tauri's pipeline
//! all share one definition. See ADR-018 for the boundary rule.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// Series declaration field: sequential mode (`series: true`) or explicit
/// wikilink order (`series: ["[[Ch 1]]", "[[Ch 2]]"]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(untagged)]
pub enum SeriesField {
    /// `series: true` — sequential mode, sort children by weight.
    Flag(bool),
    /// `series: ["[[Ch 1]]", "[[Ch 2]]"]` — explicit wikilink order.
    Ordered(Vec<String>),
}

/// Analytics configuration for script injection.
///
/// Supports two frontmatter formats:
/// - String shorthand: `analytics: "https://guo.goatcounter.com/count"` (provider auto-detected from URL)
/// - Object form: `analytics: { provider: goatcounter, url: "..." }`
#[derive(Debug, Serialize, Default, Clone)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct AnalyticsConfig {
    /// Analytics provider: "goatcounter", "umami" (default when absent)
    pub provider: Option<String>,
    /// URL — script src for Umami, count endpoint for GoatCounter
    pub url: String,
    /// Site ID for the analytics service (Umami only)
    pub site_id: Option<String>,
}

impl<'de> serde::Deserialize<'de> for AnalyticsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct AnalyticsVisitor;

        impl<'de> de::Visitor<'de> for AnalyticsVisitor {
            type Value = AnalyticsConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a URL string or an analytics config object")
            }

            fn visit_str<E: de::Error>(self, url: &str) -> Result<Self::Value, E> {
                Ok(AnalyticsConfig {
                    provider: None,
                    url: url.to_string(),
                    site_id: None,
                })
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Inner {
                    provider: Option<String>,
                    url: String,
                    site_id: Option<String>,
                }
                let inner =
                    Inner::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(AnalyticsConfig {
                    provider: inner.provider,
                    url: inner.url,
                    site_id: inner.site_id,
                })
            }
        }

        deserializer.deserialize_any(AnalyticsVisitor)
    }
}

impl AnalyticsConfig {
    /// Generate the HTML script tag for this analytics configuration.
    /// When `provider` is None, auto-detects from the URL domain.
    pub fn to_script_tag(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or_else(|| {
            if self.url.contains("goatcounter.com") {
                "goatcounter"
            } else {
                "umami"
            }
        });
        match provider {
            "goatcounter" => {
                format!(
                    r#"<script data-goatcounter="{}" async src="//gc.zgo.at/count.js"></script>"#,
                    self.url
                )
            }
            _ => {
                format!(
                    r#"<script defer src="{}" data-website-id="{}"></script>"#,
                    self.url,
                    self.site_id.as_deref().unwrap_or("")
                )
            }
        }
    }

    /// Generate a tracking pixel URL for the given path, if the provider supports it.
    /// GoatCounter's /count endpoint returns a 1x1 GIF when accessed without JavaScript,
    /// making it suitable as an `<img>` src for RSS feed tracking.
    pub fn to_pixel_url(&self, path: &str) -> Option<String> {
        let provider = self.provider.as_deref().unwrap_or_else(|| {
            if self.url.contains("goatcounter.com") {
                "goatcounter"
            } else {
                "umami"
            }
        });
        match provider {
            "goatcounter" => {
                let clean_path = if path.starts_with('/') {
                    path.to_string()
                } else {
                    format!("/{}", path) // allow:served-path-url-construct (GoatCounter analytics page path, not a framework asset URL)
                };
                Some(format!("{}?p={}", self.url, clean_path))
            }
            _ => None,
        }
    }
}

/// Frontmatter structure for parsing YAML metadata from markdown files
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct FrontMatter {
    /// Optional title override from frontmatter
    #[serde(default, deserialize_with = "deserialize_string_lenient")]
    pub title: Option<String>,
    /// Optional publication date
    pub date: Option<String>,
    /// Navigation weight for ordering (lower numbers = higher priority)
    pub weight: Option<i32>,
    /// Custom URL path override (e.g., "links" -> "/links/")
    /// Takes priority over filename-based slug generation
    pub url: Option<String>,
    /// Author name. Single name or pre-formatted list ("A and B", "A, B, and C").
    /// Captured automatically by `moss import` from JSON-LD / OpenGraph metadata.
    pub author: Option<String>,
    /// Publishing outlet name (for imported pages). `moss import` resolves this
    /// from schema.org `publisher` (via `@id` ref to an `Organization` entry)
    /// or OpenGraph `og:site_name`, falling back to the URL host.
    pub publisher: Option<String>,
    /// Linkblog target: when set, internal references to this page (cards,
    /// link rewrites, canonical, sitemap) point here instead of the local URL.
    /// The page is still built locally — direct visits to its slug still
    /// work — but the canonical home is elsewhere on the web. Pattern from
    /// [JSON Feed 1.1](https://www.jsonfeed.org/version/1.1/): `external_url`
    /// is the same as the href in a linkblog post.
    ///
    /// `moss import` populates this from the captured source URL so imported
    /// pages route every internal link (card href, wikilink rewrite, canonical,
    /// sitemap) to the outlet's URL while the local archive remains addressable
    /// at its slug.
    ///
    /// `source_url` alias — accepts existing files with the previous (one-PR)
    /// field name without breaking. Single-direction back-compat: writes use
    /// `external_url` only. Remove the alias one release after merge.
    #[serde(alias = "source_url")]
    pub external_url: Option<String>,
    /// Analytics configuration for privacy-focused analytics
    pub analytics: Option<AnalyticsConfig>,
    /// Site logo image path (rendered before site name in nav)
    pub logo: Option<String>,
    /// Cover image URL for collection pages
    pub cover: Option<String>,
    /// Explicit cover type override: "image", "video", or "iframe"
    pub cover_type: Option<String>,
    /// Whether to show in navigation
    pub nav: Option<bool>,
    /// Whether this is a draft (don't generate page)
    pub draft: Option<bool>,
    /// Whether page is unlisted (generated but hidden from lists)
    pub unlisted: Option<bool>,
    /// Page description for SEO and list previews
    pub description: Option<String>,
    /// Content tags for organization
    pub tags: Option<Vec<String>>,
    /// Whether to render child pages below content.
    /// Accepts bool (true/false) or a wikilink/path string like "[[News]]".
    /// The lenient deserializer resolves ANY non-bool string to `Some(true)`
    /// (= render children on); the folder reference itself is extracted into
    /// `children_source` by `crate::frontmatter_union::normalize_children`
    /// in the pipeline. This `Option<bool>` is the RESOLVED form the render
    /// layer consumes — its type is unchanged so no render-layer ripple.
    #[serde(default, deserialize_with = "deserialize_children_lenient")]
    pub children: Option<bool>,
    /// Wikilink reference for targeted children rendering (e.g. "[[News]]").
    /// When set, only the referenced folder's articles are rendered as
    /// children, instead of all direct children of the current page.
    pub children_source: Option<String>,
    /// Wikilink to folder whose children appear in sidebar (e.g. "[[News]]")
    pub sidebar: Option<String>,
    /// How child pages are rendered: "list" (default), "card"
    pub children_style: Option<String>,
    /// How children are grouped: "year" or "none"
    pub children_group: Option<String>,
    /// What children to include: "direct" (default), "all" descendants
    pub children_depth: Option<String>,
    /// Where to render the children feed: "body" (default) or "sidebar".
    /// Resolved at consumer; absent means body.
    pub children_in: Option<String>,
    /// Cap the children feed at N items. If truncated, a "More →" link
    /// is added. Absent = no cap.
    pub children_limit: Option<u32>,
    /// Internal: marks frontmatter that came from the deprecated `sidebar:` alias.
    /// Used by the sidebar callsite to apply the legacy default-3 limit on cross-ref.
    /// Skip-serialize so the form doesn't round-trip the synthetic flag back into the file.
    ///
    /// The sidebar callsite reads this flag (not `sidebar.is_some()`) so a
    /// conflict like `sidebar: "[[A]]" + children: "[[B]]"` — where the alias
    /// yields and warns "sidebar ignored" — actually skips the right rail
    /// rather than rendering it. Removed alongside the alias itself (#633).
    #[serde(skip_serializing, default)]
    pub _from_sidebar_alias: Option<bool>,
    /// Listing sort: axis (date/weight/title) or explicit list of child stems.
    #[serde(alias = "order")]
    pub sort: Option<crate::sort::SortField>,
    /// Series declaration: bool for prev/next chrome. Legacy list form
    /// (SeriesField::Ordered) preserved for back-compat but normalized away
    /// at deserialize time — see FrontMatter::normalize (Task 5).
    pub series: Option<SeriesField>,
    /// Override site-wide breadcrumb setting
    #[serde(default, deserialize_with = "deserialize_bool_lenient")]
    pub breadcrumb: Option<bool>,
    /// Override site-wide footer setting
    #[serde(default, deserialize_with = "deserialize_bool_lenient")]
    pub footer: Option<bool>,
    /// Frontmatter values to cascade to all descendants
    pub cascade: Option<HashMap<String, Value>>,
    /// Folder paths where this article also appears in lists
    #[serde(alias = "also")]
    pub also_in: Option<Vec<String>>,
    /// Language override (e.g., "en", "zh-hans", "zh-hant")
    pub lang: Option<String>,
    /// Translation key for linking arbitrary files as translations
    #[serde(rename = "translationKey")]
    pub translation_key: Option<String>,
    /// Whether to show comments on this page (default: true)
    pub comments: Option<bool>,
    /// Content-addressable unique identifier (first 8 chars of SHA-256 of relative path)
    #[serde(default, deserialize_with = "deserialize_string_lenient")]
    pub uid: Option<String>,
    /// Typesetting direction: "horizontal" (default) or "vertical"
    pub typesetting: Option<String>,
    /// Content width preset: "wide" or "full"
    pub content_width: Option<String>,
    /// Template layout override: "page" or "article"
    pub layout: Option<String>,
    /// URL of item being reviewed (activates review feature for this page)
    pub review_of: Option<String>,
    /// Author's rating of the reviewed item (1-5)
    pub rating: Option<u8>,
    /// Named slot this page injects into (e.g. `footer-left`).
    /// Validation against the recognized slot vocabulary happens at consumer
    /// time in the build pipeline, so authors get a deferred warning rather
    /// than a hard parse error.
    pub slot: Option<String>,
    /// Override for the email subject. When None, send uses title.
    /// When the modal's edit equals title, this field is cleared (writer
    /// can revert to "no override" by typing the title back in).
    #[serde(default)]
    pub email_subject: Option<String>,
    /// Override for the email preheader (the inbox-preview text).
    /// When None, send uses description. When the modal's edit equals
    /// description, this field is cleared.
    #[serde(default)]
    pub email_preview: Option<String>,
}

impl FrontMatter {
    /// One-time normalization after deserialize: consume legacy
    /// `series: [list]` (SeriesField::Ordered) into `sort: List + series: Flag(true)`.
    /// Idempotent. If `sort:` is already set explicitly, only the `series` field
    /// flips to Flag(true) (preserving the chrome-implied semantics).
    pub fn normalize(&mut self) {
        if let Some(SeriesField::Ordered(items)) = &self.series {
            if self.sort.is_none() {
                self.sort = Some(crate::sort::SortField::List(items.clone()));
            }
            self.series = Some(SeriesField::Flag(true));
        }
    }
}

/// One field's non-recoverable parse outcome, for build advisories and chip states (ADR-020).
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldWarning {
    /// Frontmatter key the warning is about (empty = whole-block, Phase 2 coarse).
    pub key: String,
    pub kind: FieldWarningKind,
    /// Author-facing message.
    pub message: String,
}

/// Severity of a field warning, tracking recoverability.
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldWarningKind {
    /// Value coerced to fit its field (e.g. numeric → string). Info-level.
    Coerced,
    /// Coerced but fidelity may be lost (e.g. float-like uid). Warning-level.
    Lossy,
    /// Value could not satisfy the typed schema; field/block defaulted. Error-level.
    Dropped,
}

/// Project the canonical parsed frontmatter map into the typed `FrontMatter`.
///
/// The single typed projection shared by the build (publish) and, later, the
/// editor. `serde_yaml::from_value` reuses every `#[derive(Deserialize)]` +
/// `deserialize_with` on `FrontMatter` — no per-field code — and serde_yaml
/// coerces YAML scalars, so a numeric uid/title becomes a string here.
///
/// Phase 2 contract: a value that genuinely cannot satisfy the typed schema
/// (e.g. `weight: high`) makes the whole projection fall back to
/// `FrontMatter::default()` with one `Dropped` warning. Phase 3 makes this
/// field-granular via the schema; this signature is stable across that change.
/// Pure; no I/O.
pub fn project_typed(values: &serde_yaml::Mapping) -> (FrontMatter, Vec<FieldWarning>) {
    match serde_yaml::from_value::<FrontMatter>(serde_yaml::Value::Mapping(values.clone())) {
        Ok(fm) => (fm, Vec::new()),
        Err(e) => (
            FrontMatter::default(),
            vec![FieldWarning {
                key: String::new(),
                kind: FieldWarningKind::Dropped,
                message: format!("frontmatter could not be fully parsed: {e}"),
            }],
        ),
    }
}

/// Deserialize a bool that may be a YAML string ("true"/"false") or a native bool.
/// Returns None for missing values, Some(bool) for valid values, errors for invalid strings.
pub fn deserialize_bool_lenient<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct BoolLenientVisitor;

    impl<'de> de::Visitor<'de> for BoolLenientVisitor {
        type Value = Option<bool>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a boolean or string \"true\"/\"false\"")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            match v {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(E::custom(format!("invalid bool string: {}", v))),
            }
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(BoolLenientVisitor)
}

/// Lenient deserializer for the `children` union field.
///
/// Accepts `bool` OR a string. A bool passes through; a string is resolved via
/// the SHARED `crate::frontmatter_union::normalize_children` so the build
/// pipeline and the editor agree byte-for-byte on what a value means. The folder
/// reference carried by a string is recovered separately by calling
/// `normalize_children` on the raw value in the pipeline (this deserializer only
/// produces the resolved `Option<bool>`; it cannot write the sibling
/// `children_source`). This replaces the old pre-parse YAML rewrite.
pub fn deserialize_children_lenient<'de, D>(
    deserializer: D,
) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct ChildrenLenientVisitor;

    impl<'de> de::Visitor<'de> for ChildrenLenientVisitor {
        type Value = Option<bool>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a boolean or a wikilink/path string")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let norm = crate::frontmatter_union::normalize_children(
                &serde_yaml::Value::String(v.to_string()),
            );
            Ok(Some(norm.children))
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(ChildrenLenientVisitor)
}

/// Lenient deserializer for string fields YAML may have implicitly typed as a
/// non-string scalar. `uid: 46160604` -> integer, `uid: 753659e7` -> float,
/// `title: 2024` -> integer. serde struct deserialize is atomic, so without this
/// one such field fails the WHOLE `FrontMatter` (and the pipeline blanks every
/// field). Stringify int/float/bool scalars so a numeric value can't poison its
/// neighbors. Integer round-trips exactly; a float token is lossy (YAML already
/// collapsed it to f64) — accepted because losing the whole block is worse. See
/// ADR-020.
pub fn deserialize_string_lenient<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    struct StringLenientVisitor;
    impl<'de> de::Visitor<'de> for StringLenientVisitor {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, or a number/bool YAML coerced from one")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> { Ok(Some(v.to_string())) }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> { Ok(Some(v)) }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> { Ok(Some(v.to_string())) }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> { Ok(Some(v.to_string())) }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> { Ok(Some(v.to_string())) }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> { Ok(Some(v.to_string())) }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
        fn visit_some<D2>(self, d: D2) -> Result<Self::Value, D2::Error>
        where D2: de::Deserializer<'de> { d.deserialize_any(StringLenientVisitor) }
    }
    deserializer.deserialize_any(StringLenientVisitor)
}

/// Extract a meaningful name from a frontmatter reference that may be either
/// a wikilink (`[[Departure]]`), a resolved path (`travel/departure.md`),
/// or a plain name (`Departure`).
///
/// **Note:** Wikilink resolution is centralized in `crates/moss-core/src/resolve/`.
/// Frontmatter values like sidebar, cover, and series are already resolved paths
/// by the time they reach this module. Do not add wikilink handling here.
///
/// After `resolve_frontmatter_wikilinks`, series/sidebar entries are resolved
/// to file paths.  This helper extracts a name for matching against
/// `ParsedDocument::clean_stem` or folder slugs.
///
/// For folder notes (`index.md`), returns the parent folder name since
/// the meaningful identifier is the folder, not "index".
///
/// Examples:
///   - `"[[Departure]]"` → `"Departure"` (wikilink fallback)
///   - `"travel/departure.md"` → `"departure"` (path → filename stem)
///   - `"blog/index.md"` → `"blog"` (folder note → folder name)
///   - `"news.md"` → `"news"` (root file → stem)
///   - `"Departure"` → `"Departure"` (plain name, pass-through)
#[allow(clippy::string_slice)] // char-aligned: ASCII quote chars (single-byte) + len >= 2 guard prevents [1..0] panic
pub fn frontmatter_ref_to_stem(s: &str) -> String {
    let trimmed = s.trim();

    // Strip optional surrounding quotes (simplified frontmatter preserves them)
    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    // Safety fallback: handle wikilink brackets for direct callers that
    // bypass the resolve phase (e.g. direct process_markdown_file calls).
    #[allow(clippy::string_slice)] // char-aligned: starts_with/ends_with already verified ASCII bracket chars (single-byte)
    let cleaned = if unquoted.starts_with("[[") && unquoted.ends_with("]]") {
        let inner = &unquoted[2..unquoted.len() - 2];
        // Strip leading / (Obsidian root-relative paths)
        inner.trim_start_matches('/')
    } else {
        unquoted
    };

    // If the result looks like a file path (has / or .md extension),
    // extract the meaningful name.
    if cleaned.contains('/') || cleaned.ends_with(".md") {
        #[allow(clippy::unwrap_used)] // rsplit always has at least one element
        let filename = cleaned.rsplit('/').next().unwrap_or(cleaned);
        #[allow(clippy::string_slice)] // byte index from rfind is char-aligned (ASCII dot)
        let stem = match filename.rfind('.') {
            Some(pos) if pos > 0 => &filename[..pos],
            _ => filename,
        };
        // For folder notes (index.md), use the parent folder name
        if stem == "index" {
            let parent = cleaned.rsplit('/').nth(1);
            match parent {
                Some(folder) => folder.to_string(),
                None => stem.to_string(),
            }
        } else {
            stem.to_string()
        }
    } else {
        cleaned.to_string()
    }
}

/// Translate the deprecated `sidebar:` field into the unified `children` family.
///
/// Fires when `sidebar:` is set AND there is no positive `children:` intent
/// (children unset, or `false`). Sets `children_source`, `children = true`,
/// `children_in = "sidebar"`, and the `_from_sidebar_alias` provenance flag
/// so the sidebar callsite can apply the legacy default-3 limit on cross-ref.
///
/// If `children:` is `true` or a wikilink, the alias yields and `sidebar:` is
/// ignored — explicit children intent wins. Returns deprecation/conflict
/// warnings for the build log.
pub fn apply_sidebar_alias(fm: &mut FrontMatter) -> Vec<String> {
    let Some(sidebar_ref) = fm.sidebar.clone() else {
        return Vec::new();
    };

    let mut warnings = Vec::new();
    let has_positive_children_intent =
        fm.children_source.is_some() || matches!(fm.children, Some(true));

    if has_positive_children_intent {
        warnings.push(
            "`sidebar:` ignored because `children:` is set; remove `sidebar:` and use `children_in: sidebar`"
                .to_string(),
        );
    } else {
        fm.children = Some(true);
        fm.children_source = Some(sidebar_ref.clone());
        fm.children_in = Some("sidebar".to_string());
        fm._from_sidebar_alias = Some(true);
        warnings.push(format!(
            "`sidebar:` is deprecated; use `children: {} + children_in: sidebar`",
            sidebar_ref
        ));
    }

    warnings
}

/// Detects if content uses simplified frontmatter syntax.
/// Simplified frontmatter:
/// - Does NOT start with `---`
/// - Has lines before a standalone `---` delimiter
/// - Uses format: `key` (boolean) or `key: value`
pub fn is_simplified_frontmatter(content: &str) -> bool {
    // If starts with ---, it's traditional YAML frontmatter
    if content.trim_start().starts_with("---") {
        return false;
    }
    // Check if there's a standalone --- line (not at the start)
    // but ignore --- inside ::: directive blocks (e.g., :::grid uses --- as cell separator)
    let mut in_directive = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(":::") && trimmed.len() > 3 {
            in_directive = true;
        } else if trimmed == ":::" && in_directive {
            in_directive = false;
        } else if trimmed == "---" && !in_directive {
            return true;
        }
    }
    false
}

/// Parse simplified frontmatter format into FrontMatter struct.
/// Format:
/// - Boolean flags: just the word (e.g., `nav` → nav: true)
/// - Key-value: `key: value`
/// - Comma lists: `key: a, b, c`
pub fn parse_simplified_frontmatter(content: &str) -> (FrontMatter, String) {
    let mut frontmatter = FrontMatter::default();
    let mut body_start = 0;
    let in_frontmatter = true;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // End of frontmatter
        if trimmed == "---" {
            // Calculate byte offset for the body
            body_start = content.lines()
                .take(i + 1)
                .map(|l| l.len() + 1) // +1 for newline
                .sum();
            break;
        }

        if !in_frontmatter || trimmed.is_empty() {
            continue;
        }

        // Parse the line
        if let Some(colon_pos) = trimmed.find(':') {
            // Key-value pair
            #[allow(clippy::string_slice)] // byte index from find(':') is char-aligned (ASCII colon)
            let key = trimmed[..colon_pos].trim();
            #[allow(clippy::string_slice)] // byte index from find(':') + 1 is char-aligned (ASCII colon is single-byte)
            let value = trimmed[colon_pos + 1..].trim();

            match key {
                "title" => frontmatter.title = Some(value.to_string()),
                "date" => frontmatter.date = Some(value.to_string()),
                "weight" => frontmatter.weight = value.parse().ok(),
                "url" => frontmatter.url = Some(value.to_string()),
                "cover" => frontmatter.cover = Some(value.to_string()),
                // D1: children is boolean — "true" → Some(true), "false" → Some(false)
                // D5: `list` alias removed (breaking change) — see docs/plans/2026-03-05-sidebar-redesign.md
                "children" => {
                    match value {
                        "true" | "" => frontmatter.children = Some(true),
                        "false" => frontmatter.children = Some(false),
                        v if v.starts_with("[[") && v.ends_with("]]") => {
                            // Wikilink reference: render children from the target folder
                            frontmatter.children = Some(true);
                            frontmatter.children_source = Some(v.to_string());
                        }
                        _ => {
                            eprintln!("Warning: children: \"{}\" is not valid. Use true, false, or \"[[Folder]]\".", value);
                        }
                    }
                }
                "sidebar" => frontmatter.sidebar = Some(value.to_string()),
                "children_style" => frontmatter.children_style = Some(value.to_string()),
                "children_group" => frontmatter.children_group = Some(value.to_string()),
                "children_depth" => frontmatter.children_depth = Some(value.to_string()),
                "children_in" => match value {
                    "body" | "sidebar" => frontmatter.children_in = Some(value.to_string()),
                    _ => eprintln!(
                        "Warning: children_in: \"{}\" is not valid. Use \"body\" or \"sidebar\".",
                        value
                    ),
                },
                "children_limit" => frontmatter.children_limit = value.parse().ok(),
                "description" => frontmatter.description = Some(value.to_string()),
                "lang" => frontmatter.lang = Some(value.to_string()),
                "translationKey" | "translation_key" => {
                    frontmatter.translation_key = Some(value.to_string())
                }
                "also" | "also_in" => {
                    let items: Vec<String> = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !items.is_empty() {
                        frontmatter.also_in = Some(items);
                    }
                }
                "tags" => {
                    let items: Vec<String> = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !items.is_empty() {
                        frontmatter.tags = Some(items);
                    }
                }
                // Handle boolean with explicit value
                "nav" => frontmatter.nav = Some(value == "true" || value.is_empty()),
                "draft" => frontmatter.draft = Some(value == "true" || value.is_empty()),
                "unlisted" => frontmatter.unlisted = Some(value == "true" || value.is_empty()),
                "breadcrumb" => frontmatter.breadcrumb = Some(value == "true" || value.is_empty()),
                "footer" => frontmatter.footer = Some(value == "true" || value.is_empty()),
                "comments" => frontmatter.comments = Some(value == "true" || value.is_empty()),
                "slot" => {
                    if !value.is_empty() {
                        // Validation against known slot names happens at
                        // consumer time in `build::footer::collect_footer_slots`,
                        // so authors get a deferred warning rather than a
                        // hard parse error.
                        frontmatter.slot = Some(value.to_string());
                    }
                }
                "email_subject" => frontmatter.email_subject = Some(value.to_string()),
                "email_preview" => frontmatter.email_preview = Some(value.to_string()),
                _ => {} // Unknown key, ignore
            }
        } else {
            // Boolean flag (just the word)
            match trimmed {
                "nav" => frontmatter.nav = Some(true),
                "draft" => frontmatter.draft = Some(true),
                "unlisted" => frontmatter.unlisted = Some(true),
                "breadcrumb" => frontmatter.breadcrumb = Some(true),
                "footer" => frontmatter.footer = Some(true),
                "comments" => frontmatter.comments = Some(true),
                "children" => frontmatter.children = Some(true),
                _ => {} // Unknown flag, ignore
            }
        }
    }

    #[allow(clippy::string_slice)] // body_start is a byte offset computed by summing line lengths + newlines (ASCII-safe)
    let body = if body_start < content.len() {
        content[body_start..].to_string()
    } else {
        String::new()
    };

    (frontmatter, body)
}

/// Compute the output URL path for a source file.
/// This is the single source of truth for file-tree → page-tree mapping.
///
/// All directory segments and the file basename go through [`crate::slug::generate_slug`],
/// so URLs are always lowercase kebab-case regardless of how the source files
/// and folders are cased on disk.
///
/// # Arguments
/// * `file_path` - Relative path from site root (e.g., "posts/hello.md")
/// * `is_index_file` - Whether this file is the index/home file for its folder
/// * `frontmatter_url` - Optional `url` override from frontmatter
/// * `clean_stem` - Language-suffix-stripped filename stem (e.g., "hello" from "hello.zh.md")
pub fn compute_url_path(
    file_path: &str,
    is_index_file: bool,
    frontmatter_url: Option<&str>,
    clean_stem: &str,
) -> String {
    use std::path::Path;
    use crate::slug::{generate_slug, slugify_path_segments};

    if is_index_file {
        let parent_path = Path::new(file_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        if parent_path.is_empty() {
            "index.html".to_string()
        } else if let Some(custom_url) = frontmatter_url {
            // url override replaces the last segment of the parent path
            let grandparent = Path::new(parent_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let slug = generate_slug(custom_url);
            if grandparent.is_empty() {
                format!("{}/index.html", slug)
            } else {
                format!("{}/{}/index.html", slugify_path_segments(grandparent), slug)
            }
        } else {
            format!("{}/index.html", slugify_path_segments(parent_path))
        }
    } else {
        // Check for frontmatter url override first
        let slug = if let Some(custom_url) = frontmatter_url {
            generate_slug(custom_url)
        } else {
            // Use clean stem (language suffix stripped)
            generate_slug(clean_stem)
        };

        // Preserve directory structure, output as slug/index.html (pretty URL)
        let parent_path = Path::new(file_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");

        if parent_path.is_empty() {
            format!("{}/index.html", slug)
        } else {
            format!("{}/{}/index.html", slugify_path_segments(parent_path), slug)
        }
    }
}

#[cfg(test)]
mod project_typed_tests {
    use super::*;

    fn map_of(pairs: &[(&str, serde_yaml::Value)]) -> serde_yaml::Mapping {
        let mut m = serde_yaml::Mapping::new();
        for (k, v) in pairs {
            m.insert(serde_yaml::Value::String((*k).to_string()), v.clone());
        }
        m
    }

    #[test]
    fn project_typed_clean_map_no_warnings() {
        use serde_yaml::Value;
        let m = map_of(&[
            ("title", Value::String("Hello".into())),
            ("date", Value::String("2025-05-28".into())),
        ]);
        let (fm, warnings) = project_typed(&m);
        assert_eq!(fm.title.as_deref(), Some("Hello"));
        assert_eq!(fm.date.as_deref(), Some("2025-05-28"));
        assert!(warnings.is_empty(), "clean frontmatter yields no warnings");
    }

    #[test]
    fn project_typed_numeric_uid_coerces() {
        use serde_yaml::Value;
        // The canonical parse produced a YAML integer for uid (the real-world bug).
        let m = map_of(&[
            ("title", Value::String("Paper".into())),
            ("uid", Value::Number(serde_yaml::Number::from(46160604u64))),
        ]);
        let (fm, warnings) = project_typed(&m);
        assert_eq!(fm.uid.as_deref(), Some("46160604"));
        assert_eq!(fm.title.as_deref(), Some("Paper"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn project_typed_ignores_unknown_fields() {
        use serde_yaml::Value;
        let m = map_of(&[
            ("title", Value::String("T".into())),
            ("syndicated", Value::String("https://example.com".into())),
            ("some_plugin_field", Value::Number(serde_yaml::Number::from(7u64))),
        ]);
        let (fm, warnings) = project_typed(&m);
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert!(warnings.is_empty(), "unknown fields are ignored, not errors");
    }

    #[test]
    fn project_typed_unrepresentable_field_falls_back_with_warning() {
        use serde_yaml::Value;
        // `weight: high` cannot become i32. Phase 2 coarse behavior: whole struct
        // defaults + one Dropped warning. (Phase 3 makes this field-granular.)
        let m = map_of(&[
            ("title", Value::String("T".into())),
            ("weight", Value::String("high".into())),
        ]);
        let (fm, warnings) = project_typed(&m);
        assert_eq!(fm.title, None, "Phase 2 coarse fallback defaults the whole struct");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].kind, FieldWarningKind::Dropped);
        assert!(!warnings[0].message.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // serde_yaml coerces numbers→String on its own, so these document behavior; the real
    // build-path guards are the *_via_json_path tests below.
    #[test]
    fn numeric_title_coerces_to_string() {
        let fm: FrontMatter = serde_yaml::from_str("title: 2024\ndate: 2024-01-01\n").expect("parse");
        assert_eq!(fm.title.as_deref(), Some("2024"));
        assert_eq!(fm.date.as_deref(), Some("2024-01-01"));
    }

    #[test]
    fn numeric_uid_coerces_and_preserves_siblings() {
        let yaml = "title: Kept Title\nuid: 46160604\ndate: 2025-05-28\n";
        let fm: FrontMatter = serde_yaml::from_str(yaml).expect("must not fail to parse");
        assert_eq!(fm.uid.as_deref(), Some("46160604"), "integer uid round-trips exactly");
        assert_eq!(fm.title.as_deref(), Some("Kept Title"), "sibling title must survive");
        assert_eq!(fm.date.as_deref(), Some("2025-05-28"), "sibling date must survive");
    }
    #[test]
    fn float_like_uid_does_not_blank_struct() {
        let yaml = "title: T\nuid: 753659e7\n";
        let fm: FrontMatter = serde_yaml::from_str(yaml).expect("must not fail to parse");
        assert!(fm.uid.is_some(), "float-like uid yields some string");
        assert_eq!(fm.title.as_deref(), Some("T"));
    }
    #[test]
    fn string_uid_unchanged() {
        let fm: FrontMatter = serde_yaml::from_str("title: T\nuid: 54ddc5c0\n").expect("parse");
        assert_eq!(fm.uid.as_deref(), Some("54ddc5c0"));
    }
    #[test]
    fn missing_uid_is_none() {
        let fm: FrontMatter = serde_yaml::from_str("title: T\n").expect("parse");
        assert_eq!(fm.uid, None);
        assert_eq!(fm.title.as_deref(), Some("T"));
    }
    #[test]
    fn null_uid_is_none() {
        let fm: FrontMatter = serde_yaml::from_str("title: T\nuid:\n").expect("parse");
        assert_eq!(fm.uid, None);
    }

    // These mirror the BUILD path: gray_matter's Pod::deserialize lowers to
    // serde_json::Value (Pod::Integer => json!(val)) then serde_json::from_value,
    // which — unlike serde_yaml — does NOT coerce numbers to String. Without
    // deserialize_string_lenient these FAIL ("invalid type: integer, expected a
    // string") and the whole FrontMatter would blank. See ADR-020.
    #[test]
    fn numeric_uid_via_json_path_coerces_and_preserves_siblings() {
        let v = serde_json::json!({ "title": "Kept Title", "uid": 46160604u64, "date": "2025-05-28" });
        let fm: FrontMatter = serde_json::from_value(v).expect("build path must not fail on numeric uid");
        assert_eq!(fm.uid.as_deref(), Some("46160604"));
        assert_eq!(fm.title.as_deref(), Some("Kept Title"));
        assert_eq!(fm.date.as_deref(), Some("2025-05-28"));
    }
    #[test]
    fn numeric_title_via_json_path_coerces() {
        let v = serde_json::json!({ "title": 2024u64, "date": "2024-01-01" });
        let fm: FrontMatter = serde_json::from_value(v).expect("build path must not fail on numeric title");
        assert_eq!(fm.title.as_deref(), Some("2024"));
        assert_eq!(fm.date.as_deref(), Some("2024-01-01"));
    }

    #[test]
    fn frontmatter_ref_to_stem_single_char_quote_no_panic() {
        // Regression: single-char `"` or `'` must not panic at [1..0]
        assert_eq!(frontmatter_ref_to_stem("\""), "\"");
        assert_eq!(frontmatter_ref_to_stem("'"), "'");
    }

    #[test]
    fn series_field_flag_roundtrips() {
        let v: SeriesField = serde_yaml::from_str("true").unwrap();
        assert!(matches!(v, SeriesField::Flag(true)));
        let v: SeriesField = serde_yaml::from_str("false").unwrap();
        assert!(matches!(v, SeriesField::Flag(false)));
    }

    #[test]
    fn series_field_ordered_roundtrips() {
        let v: SeriesField = serde_yaml::from_str(r#"["[[Ch 1]]", "[[Ch 2]]"]"#).unwrap();
        assert!(matches!(v, SeriesField::Ordered(ref items) if items.len() == 2));
    }
}

#[cfg(test)]
mod url_path_tests {
    use super::*;

    // Tests for slugify_path_segments and compute_url_path
    // (folder-name URL slugify regression coverage)

    #[test]
    fn test_slugify_path_segments_lowercase_passthrough() {
        assert_eq!(crate::slug::slugify_path_segments("posts"), "posts");
        assert_eq!(crate::slug::slugify_path_segments("posts/2024"), "posts/2024");
    }

    #[test]
    fn test_slugify_path_segments_title_case_lowercased() {
        assert_eq!(crate::slug::slugify_path_segments("News"), "news");
        assert_eq!(crate::slug::slugify_path_segments("Projects"), "projects");
    }

    #[test]
    fn test_slugify_path_segments_spaces_kebabed() {
        assert_eq!(crate::slug::slugify_path_segments("My Section"), "my-section");
        assert_eq!(
            crate::slug::slugify_path_segments("News/Sub Section"),
            "news/sub-section"
        );
    }

    #[test]
    fn test_slugify_path_segments_empty() {
        assert_eq!(crate::slug::slugify_path_segments(""), "");
    }

    #[test]
    fn test_slugify_path_segments_unicode_preserved() {
        assert_eq!(crate::slug::slugify_path_segments("文章"), "文章");
        assert_eq!(crate::slug::slugify_path_segments("文章/Hello World"), "文章/hello-world");
    }

    #[test]
    fn test_compute_url_path_top_level_file() {
        // Top-level file: filename slugified, no parent.
        assert_eq!(
            compute_url_path("Funding.md", false, None, "Funding"),
            "funding/index.html"
        );
        assert_eq!(
            compute_url_path("Code of Conduct.md", false, None, "Code of Conduct"),
            "code-of-conduct/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_file_in_lowercase_folder() {
        // Regression: pre-existing lowercase folder still produces lowercase URL.
        assert_eq!(
            compute_url_path("posts/hello.md", false, None, "hello"),
            "posts/hello/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_file_in_title_case_folder() {
        // Bug fix: Title-Case folder must be lowercased in the URL.
        assert_eq!(
            compute_url_path(
                "News/2026-04-22-morbidelli.md",
                false,
                None,
                "2026-04-22-morbidelli"
            ),
            "news/2026-04-22-morbidelli/index.html"
        );
        assert_eq!(
            compute_url_path("Projects/giant-planets.md", false, None, "giant-planets"),
            "projects/giant-planets/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_file_in_nested_title_case_folders() {
        assert_eq!(
            compute_url_path(
                "My Section/Sub Page/page.md",
                false,
                None,
                "page"
            ),
            "my-section/sub-page/page/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_index_file_in_title_case_folder() {
        // Bug fix: folder-file (e.g., News/News.md) URL must be /news/.
        assert_eq!(
            compute_url_path("News/News.md", true, None, "News"),
            "news/index.html"
        );
        assert_eq!(
            compute_url_path("Projects/Projects.md", true, None, "Projects"),
            "projects/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_index_file_in_lowercase_folder() {
        // Regression: lowercase folder index unchanged.
        assert_eq!(
            compute_url_path("posts/index.md", true, None, "index"),
            "posts/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_root_index_file() {
        // Root index has no parent path; stays "index.html".
        assert_eq!(
            compute_url_path("index.md", true, None, "index"),
            "index.html"
        );
    }

    #[test]
    fn test_compute_url_path_url_override_with_title_case_grandparent() {
        // url: override slugifies to its own segment; the grandparent path
        // is also slugified so a Title-Case grandparent produces lowercase URL.
        assert_eq!(
            compute_url_path(
                "Section/Old Name/page.md",
                true,
                Some("New Name"),
                "page"
            ),
            "section/new-name/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_url_override_no_grandparent() {
        // url: override at top level (no grandparent) — slug is the only segment.
        assert_eq!(
            compute_url_path("News/index.md", true, Some("Custom Name"), "index"),
            "custom-name/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_non_index_with_url_override_in_title_case_folder() {
        // Non-index file with its own `url:` override, sitting in a Title-Case
        // folder. The override slug becomes the file segment; the parent folder
        // segment is independently slugified.
        assert_eq!(
            compute_url_path(
                "News/page.md",
                false,
                Some("Custom Slug"),
                "page"
            ),
            "news/custom-slug/index.html"
        );
    }

    #[test]
    fn test_compute_url_path_idempotent_for_already_lowercase() {
        // The fix must not break paths that were already correct.
        assert_eq!(
            compute_url_path("blog/post.md", false, None, "post"),
            "blog/post/index.html"
        );
        assert_eq!(
            compute_url_path("blog/index.md", true, None, "index"),
            "blog/index.html"
        );
    }
}
