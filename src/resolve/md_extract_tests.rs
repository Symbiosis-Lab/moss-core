use super::*;

#[test]
fn wikilink_stem() {
    let src = "See [[note]] for details.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "note");
    assert_eq!(refs[0].syntax, RefSyntax::WikilinkStem);
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[note]]");
}

#[test]
fn wikilink_path() {
    let src = "See [[a/b]] here.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "a/b");
    assert_eq!(refs[0].syntax, RefSyntax::WikilinkPath);
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[a/b]]");
}

#[test]
fn wikilink_stem_embed() {
    let src = "![[x]] is an embed.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "x");
    assert_eq!(refs[0].syntax, RefSyntax::WikilinkStemEmbed);
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[x]]");
}

#[test]
fn wikilink_path_embed() {
    let src = "![[a/b]] embedded.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "a/b");
    assert_eq!(refs[0].syntax, RefSyntax::WikilinkPathEmbed);
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[a/b]]");
}

#[test]
fn wikilink_aliased() {
    let src = "See [[stem|Display]] here.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "stem");
    assert!(
        matches!(&refs[0].syntax, RefSyntax::WikilinkAliased { display } if display == "Display")
    );
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[stem|Display]]");
}

#[test]
fn wikilink_aliased_embed() {
    let src = "![[stem|500]] wide embed.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "stem");
    assert!(
        matches!(&refs[0].syntax, RefSyntax::WikilinkAliasedEmbed { display } if display == "500")
    );
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "![[stem|500]]");
}

#[test]
fn markdown_link() {
    let src = "Click [here](page.md) now.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "page.md");
    assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownLink { label } if label == "here"));
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[here](page.md)");
}

#[test]
fn markdown_image() {
    let src = "![alt text](img.png) here.";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "img.png");
    assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownImage { alt } if alt == "alt text"));
    assert_eq!(
        &src[refs[0].byte_from..refs[0].byte_to],
        "![alt text](img.png)"
    );
}

#[test]
fn external_link_included() {
    let src = "[foo](https://example.com)";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "https://example.com");
    assert!(matches!(&refs[0].syntax, RefSyntax::MarkdownLink { .. }));
}

#[test]
fn skip_fenced_code_block() {
    let src = "```\n[[note]]\n```\nAfter.";
    let refs = extract_md_references(src);
    assert_eq!(
        refs.len(),
        0,
        "wikilink inside fenced block should be skipped"
    );
}

#[test]
fn ref_after_fence_is_found() {
    // Regression: the closing-fence line must be advanced past, otherwise
    // the backtick handler swallows the rest of the file and drops every
    // reference after a fenced block.
    let src = "```\n[[skip]]\n```\n[[find]]";
    let refs = extract_md_references(src);
    assert_eq!(
        refs.len(),
        1,
        "exactly the ref after the fence is found, got: {:?}",
        refs
    );
    assert_eq!(refs[0].text, "find");
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[find]]");
}

#[test]
fn backslash_escaped_refs_are_skipped() {
    // `\[[note]]` and `\[t](p)` are escaped and must NOT be extracted.
    let src = "Escaped \\[[note]] and \\[t](p.md) but [[real]] counts.";
    let refs = extract_md_references(src);
    assert_eq!(
        refs.len(),
        1,
        "only the unescaped ref should be found, got: {:?}",
        refs
    );
    assert_eq!(refs[0].text, "real");
}

#[test]
fn skip_inline_code_span() {
    let src = "In `` [[note]] `` code.";
    let refs = extract_md_references(src);
    assert_eq!(
        refs.len(),
        0,
        "wikilink inside inline code should be skipped"
    );
}

#[test]
fn multiple_refs_byte_offsets() {
    let src = "[[a]] and [[b]]";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 2);
    assert_eq!(&src[refs[0].byte_from..refs[0].byte_to], "[[a]]");
    assert_eq!(&src[refs[1].byte_from..refs[1].byte_to], "[[b]]");
}

#[test]
fn aliased_embed_preserves_alias() {
    // ![[image.png|600]] — pothole is "600"
    let src = "![[image.png|600]]";
    let refs = extract_md_references(src);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].text, "image.png");
    assert!(
        matches!(&refs[0].syntax, RefSyntax::WikilinkAliasedEmbed { display } if display == "600")
    );
}
