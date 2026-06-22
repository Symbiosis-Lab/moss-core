use moss_core::contract::components::COMPONENTS;
use moss_core::contract::describe::{DescribePayload, DESCRIBE_SCHEMA_VERSION, MOSS_HTML_VERSION};
use moss_core::contract::tokens::{load_tokens, parse_tokens};

#[test]
fn describe_payload_serializes_with_versions() {
    let tokens = load_tokens().expect("tokens");
    let payload = DescribePayload::new(&tokens);
    let json = serde_json::to_value(&payload).expect("serialize");

    assert_eq!(json["describe_schema_version"], DESCRIBE_SCHEMA_VERSION);
    assert_eq!(json["moss_html_version"], MOSS_HTML_VERSION);
    assert!(json["tokens"].is_object());
    assert!(json["components"].is_array());
    // Payload must remain complete — no filtering in the JSON output.
    assert_eq!(json["components"].as_array().unwrap().len(), COMPONENTS.len());
}

#[test]
fn describe_payload_includes_token_groups() {
    let tokens = load_tokens().expect("tokens");
    let payload = DescribePayload::new(&tokens);
    let json = serde_json::to_value(&payload).expect("serialize");

    let tokens_obj = json["tokens"].as_object().expect("tokens object");
    assert!(tokens_obj.contains_key("typography"));
    assert!(tokens_obj.contains_key("color"));
}

#[test]
fn describe_payload_authorable_flag_is_set_correctly() {
    let tokens = load_tokens().expect("tokens");
    let payload = DescribePayload::new(&tokens);
    let json = serde_json::to_value(&payload).expect("serialize");

    let components = json["components"].as_array().expect("components array");

    // At least one component must have authorable == true.
    assert!(
        components.iter().any(|c| c["authorable"] == true),
        "at least one component must have authorable == true"
    );

    // moss-grid must be authorable (it is a ShortcodeKind::Grid root class).
    let moss_grid = components
        .iter()
        .find(|c| c["class"] == "moss-grid")
        .expect("moss-grid must be in payload");
    assert_eq!(
        moss_grid["authorable"], true,
        "moss-grid must have authorable == true"
    );

    // moss-card is NOT authorable (it is not a shortcode root class).
    let moss_card = components
        .iter()
        .find(|c| c["class"] == "moss-card")
        .expect("moss-card must be in payload");
    assert_eq!(
        moss_card["authorable"], false,
        "moss-card must have authorable == false"
    );
}

#[test]
fn describe_token_json_includes_dark_value() {
    let json = r##"{
        "$order": ["color"],
        "color": { "moss-color-bg": {"$type":"color",
            "$value":{"light":"#faf8f5","dark":"#1c1914"}} }
    }"##;
    let tokens = parse_tokens(json).unwrap();
    let payload = DescribePayload::new(&tokens);
    let bg = payload.tokens.get("color").unwrap().iter()
        .find(|t| t.name == "moss-color-bg").unwrap();
    assert_eq!(bg.dark_value, Some("#1c1914"));
}
