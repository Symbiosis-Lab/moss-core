//! Serializable payload for `moss describe --json`.
//!
//! The JSON shape is itself a contract — version it independently of
//! `moss_html_version`. Bumping `describe_schema_version` is required for
//! any breaking change to the JSON envelope.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::ast::shortcode::ShortcodeKind;
use crate::contract::components::{COMPONENTS, Status};
use crate::contract::tokens::Tokens;

pub const DESCRIBE_SCHEMA_VERSION: u32 = 2;
pub const MOSS_HTML_VERSION: u32 = 1;

#[derive(Serialize)]
pub struct DescribePayload<'a> {
    pub describe_schema_version: u32,
    pub moss_html_version: u32,
    pub moss_binary_version: &'static str,
    pub tokens: BTreeMap<&'a str, Vec<TokenJson<'a>>>,
    pub components: Vec<ComponentJson>,
}

#[derive(Serialize)]
pub struct TokenJson<'a> {
    pub name: &'a str,
    pub value: &'a str,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_hint: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'a str>,
}

#[derive(Serialize)]
pub struct ComponentJson {
    pub class: &'static str,
    pub kind: &'static str,
    pub parent: &'static str,
    pub data_attrs: Vec<DataAttrJson>,
    pub example_html: &'static str,
    pub example_markdown: &'static str,
    pub status: &'static str,
    pub since: &'static str,
    pub description: &'static str,
    /// True iff this class is the root class of an authorable shortcode
    /// (i.e. it appears in `ShortcodeKind::all().map(|k| k.root_class())`).
    /// Agents can use this flag to distinguish the 6 author-facing shortcodes
    /// from the broader theme vocabulary.
    pub authorable: bool,
}

#[derive(Serialize)]
pub struct DataAttrJson {
    pub name: &'static str,
    pub values: &'static [&'static str],
    pub default: &'static str,
    pub description: &'static str,
}

impl<'a> DescribePayload<'a> {
    pub fn new(tokens: &'a Tokens) -> Self {
        let mut tokens_map: BTreeMap<&str, Vec<TokenJson>> = BTreeMap::new();
        for group in &tokens.groups {
            let entries: Vec<TokenJson> = group
                .entries
                .iter()
                .map(|t| TokenJson {
                    name: &t.name,
                    value: &t.value,
                    type_hint: t.type_hint.as_deref(),
                    description: t.description.as_deref(),
                })
                .collect();
            tokens_map.insert(&group.name, entries);
        }

        let authorable: std::collections::HashSet<&'static str> =
            ShortcodeKind::all().map(|k| k.root_class()).collect();

        let components: Vec<ComponentJson> = COMPONENTS
            .iter()
            .map(|c| ComponentJson {
                class: c.class,
                kind: c.kind,
                parent: c.parent,
                data_attrs: c
                    .data_attrs
                    .iter()
                    .map(|a| DataAttrJson {
                        name: a.name,
                        values: a.values,
                        default: a.default,
                        description: a.description,
                    })
                    .collect(),
                example_html: c.example_html,
                example_markdown: c.example_markdown,
                status: match c.status {
                    Status::Confirmed => "confirmed",
                    Status::Emerging => "emerging",
                    Status::Retired => "retired",
                },
                since: c.since,
                description: c.description,
                authorable: authorable.contains(c.class),
            })
            .collect();

        DescribePayload {
            describe_schema_version: DESCRIBE_SCHEMA_VERSION,
            moss_html_version: MOSS_HTML_VERSION,
            moss_binary_version: env!("CARGO_PKG_VERSION"),
            tokens: tokens_map,
            components,
        }
    }
}
