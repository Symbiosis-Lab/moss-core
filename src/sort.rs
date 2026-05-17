//! Folder-listing sort: types and inference cascade.
//!
//! Pure Rust, zero I/O. Consumed by:
//!   - the build pipeline (scan pass, card renderer, series-nav)
//!   - the editor form (to show "inferred: date" next to undeclared sort:)
//!
//! See docs/plans/2026-05-17-listing-sort-and-embeds-design.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum SortAxis {
    Date,
    Weight,
    Title,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(untagged)]
pub enum SortField {
    Axis(SortAxis),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResolvedSort {
    pub axis: SortAxis,
    pub explicit_order: Option<Vec<String>>,
    /// Default value of `series:` chrome. True iff axis == Weight OR explicit_order is Some.
    pub series_default: bool,
}

/// Minimal document trait for sort inference. Both src-tauri's
/// ParsedDocument and the editor's in-memory document model implement this.
pub trait SortableDoc {
    fn url_path(&self) -> &str;
    fn date(&self) -> Option<&str>;
    fn weight(&self) -> Option<i32>;
    fn declared_sort(&self) -> Option<&SortField>;
    fn clean_stem(&self) -> &str;
}

const DATE_FRACTION_THRESHOLD: f32 = 0.8;

pub fn resolve_folder_sort<D: SortableDoc>(
    folder: &D,
    children: &[&D],
) -> ResolvedSort {
    let article_children: Vec<&&D> = children
        .iter()
        .filter(|c| !c.url_path().ends_with("/index.html"))
        .collect();

    let (axis, explicit_order) = match folder.declared_sort() {
        Some(SortField::Axis(a)) => (*a, None),
        Some(SortField::List(items)) => (infer_axis(&article_children), Some(items.clone())),
        None => (infer_axis(&article_children), None),
    };

    let series_default = matches!(axis, SortAxis::Weight) || explicit_order.is_some();

    ResolvedSort { axis, explicit_order, series_default }
}

fn infer_axis<D: SortableDoc>(article_children: &[&&D]) -> SortAxis {
    if article_children.is_empty() {
        return SortAxis::Title;
    }
    if article_children.iter().any(|c| c.weight().is_some()) {
        return SortAxis::Weight;
    }
    let total = article_children.len() as f32;
    let dated = article_children.iter().filter(|c| c.date().is_some()).count() as f32;
    if dated / total >= DATE_FRACTION_THRESHOLD {
        return SortAxis::Date;
    }
    SortAxis::Title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_field_parses_axis_strings() {
        assert!(matches!(serde_yaml::from_str::<SortField>("date").unwrap(), SortField::Axis(SortAxis::Date)));
        assert!(matches!(serde_yaml::from_str::<SortField>("weight").unwrap(), SortField::Axis(SortAxis::Weight)));
        assert!(matches!(serde_yaml::from_str::<SortField>("title").unwrap(), SortField::Axis(SortAxis::Title)));
    }

    #[test]
    fn sort_field_parses_list() {
        let f: SortField = serde_yaml::from_str("[intro, setup, advanced]").unwrap();
        match f {
            SortField::List(items) => assert_eq!(items, vec!["intro", "setup", "advanced"]),
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn sort_field_rejects_unknown_axis() {
        assert!(serde_yaml::from_str::<SortField>("random").is_err());
    }
}

#[cfg(test)]
mod inference_tests {
    use super::*;

    #[derive(Debug, Default)]
    struct TestDoc {
        url: String,
        stem: String,
        date_v: Option<String>,
        weight_v: Option<i32>,
        sort_v: Option<SortField>,
    }
    impl SortableDoc for TestDoc {
        fn url_path(&self) -> &str { &self.url }
        fn date(&self) -> Option<&str> { self.date_v.as_deref() }
        fn weight(&self) -> Option<i32> { self.weight_v }
        fn declared_sort(&self) -> Option<&SortField> { self.sort_v.as_ref() }
        fn clean_stem(&self) -> &str { &self.stem }
    }
    fn art(stem: &str, date: Option<&str>, w: Option<i32>) -> TestDoc {
        TestDoc {
            url: format!("{}.html", stem),
            stem: stem.into(),
            date_v: date.map(|s| s.into()),
            weight_v: w,
            sort_v: None,
        }
    }
    fn folder(url: &str, sort: Option<SortField>) -> TestDoc {
        TestDoc { url: url.into(), sort_v: sort, ..Default::default() }
    }

    #[test]
    fn explicit_axis_wins() {
        let f = folder("blog/index.html", Some(SortField::Axis(SortAxis::Title)));
        let a = art("a", Some("2025-01-01"), None);
        let r = resolve_folder_sort(&f, &[&a]);
        assert_eq!(r.axis, SortAxis::Title);
        assert!(r.explicit_order.is_none());
        assert!(!r.series_default);
    }

    #[test]
    fn explicit_list_implies_chrome_on() {
        let f = folder("blog/index.html", Some(SortField::List(vec!["a".into(), "b".into()])));
        let a = art("a", None, None);
        let b = art("b", None, None);
        let r = resolve_folder_sort(&f, &[&a, &b]);
        assert!(r.series_default, "explicit list-form sort implies series chrome on");
        assert_eq!(r.explicit_order.as_ref().unwrap().len(), 2);
        assert_eq!(r.axis, SortAxis::Title, "tail axis inferred from undated children");
    }

    #[test]
    fn weight_present_infers_weight() {
        let f = folder("docs/index.html", None);
        let a = art("intro", None, Some(10));
        let b = art("advanced", None, Some(20));
        let r = resolve_folder_sort(&f, &[&a, &b]);
        assert_eq!(r.axis, SortAxis::Weight);
        assert!(r.series_default, "weight axis implies series chrome on");
    }

    #[test]
    fn dates_above_threshold_infer_date() {
        let f = folder("blog/index.html", None);
        let a = art("a", Some("2025-01-01"), None);
        let b = art("b", Some("2025-02-01"), None);
        let c = art("c", Some("2025-03-01"), None);
        let d = art("d", Some("2025-04-01"), None);
        let e = art("e", None, None);
        let r = resolve_folder_sort(&f, &[&a, &b, &c, &d, &e]);
        assert_eq!(r.axis, SortAxis::Date);  // 4/5 = 0.8 == threshold
        assert!(!r.series_default);
    }

    #[test]
    fn dates_below_threshold_fallback_to_title() {
        let f = folder("projects/index.html", None);
        let a = art("a", Some("2025-01-01"), None);
        let b = art("b", None, None);
        let c = art("c", None, None);
        let d = art("d", None, None);
        let e = art("e", None, None);
        let r = resolve_folder_sort(&f, &[&a, &b, &c, &d, &e]);
        assert_eq!(r.axis, SortAxis::Title);  // 1/5 = 0.2 < 0.8
    }

    #[test]
    fn weight_beats_date() {
        let f = folder("hybrid/index.html", None);
        let a = art("a", Some("2025-01-01"), Some(1));
        let b = art("b", Some("2025-02-01"), None);
        let r = resolve_folder_sort(&f, &[&a, &b]);
        assert_eq!(r.axis, SortAxis::Weight);
    }

    #[test]
    fn subfolders_excluded_from_inference() {
        let f = folder("root/index.html", None);
        let article = art("welcome", None, None);
        let sub_a = folder("root/news/index.html", None);
        let sub_b = folder("root/projects/index.html", None);
        let r = resolve_folder_sort(&f, &[&article, &sub_a, &sub_b]);
        assert_eq!(r.axis, SortAxis::Title);
    }

    #[test]
    fn chps_style_root_with_only_subfolders_falls_to_title() {
        let f = folder("root/index.html", None);
        let sub_a = folder("root/news/index.html", None);
        let sub_b = folder("root/projects/index.html", None);
        let r = resolve_folder_sort(&f, &[&sub_a, &sub_b]);
        assert_eq!(r.axis, SortAxis::Title);
    }

    #[test]
    fn empty_folder_defaults_to_title() {
        let f = folder("empty/index.html", None);
        let r = resolve_folder_sort::<TestDoc>(&f, &[]);
        assert_eq!(r.axis, SortAxis::Title);
    }
}
