//! Regression: CJK-friendly strong emphasis through moss's shared parser.
//!
//! Reported 2026-07-26 on `zhaozhichen.mosspub.com/linear-attention`: three
//! `**…**` spans rendered as literal `**`. Cause is CommonMark's emphasis
//! *flanking* rule — a closing `**` that sits between a CJK punctuation mark
//! (`。`/`：`/`”`) and a following CJK ideograph is not "right-flanking" (CJK
//! prose has no ASCII space there), so the span never closes.
//!
//! moss opts into pulldown-cmark's `ENABLE_CJK_FRIENDLY_EMPHASIS`
//! (pulldown-cmark#1059, implementing the `tats-u/markdown-cjk-friendly`
//! amendment to CommonMark 0.31.2) from the single [`moss_core::ast::parser_options`]
//! constructor. The extension is backward-compatible: it changes output only
//! for CJK-adjacent delimiters and is identical on every existing CommonMark
//! example, which the ASCII control below pins.
//!
//! These vectors are moss's real reported cases. Asserting on events (not HTML)
//! matches moss-core, which builds pulldown without the `html` feature.

use pulldown_cmark::{Event, Parser, Tag};

/// True when moss's shared parser recognizes a strong-emphasis span in `md`.
fn has_strong(md: &str) -> bool {
    Parser::new_ext(md, moss_core::ast::parser_options(false))
        .any(|ev| matches!(ev, Event::Start(Tag::Strong)))
}

#[test]
fn cjk_bold_closes_between_punctuation_and_ideograph() {
    // Live case 1: closing `**` flanked by `。` (CJK full stop) then `中`.
    assert!(
        has_strong("**这种重述带来了新的视角。**中间状态"),
        "full-stop → ideograph must close bold"
    );
    // Live case 3: curly-quote span, `。` close then a CJK ideograph.
    assert!(
        has_strong("**\u{201c}层级结构\u{201d}再一次触动了我的物理学神经。**后面"),
        "curly-quoted span must close bold"
    );
    // Colon close then ideograph (from the same article's middle paragraph).
    assert!(
        has_strong("原本一次表象**切换：**把它显式化"),
        "colon → ideograph must close bold"
    );
}

#[test]
fn ascii_bold_unaffected() {
    // Backward-compatibility guard: ordinary CommonMark strong emphasis,
    // where the CJK extension must not change anything.
    assert!(has_strong("**A sentence.** next"), "ascii sentence bold");
    assert!(has_strong("prefix **bold** suffix"), "ascii inline bold");
}
