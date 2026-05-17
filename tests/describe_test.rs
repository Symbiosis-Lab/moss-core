use moss_core::contract::components::COMPONENTS;
use moss_core::contract::describe::{DescribePayload, DESCRIBE_SCHEMA_VERSION, MOSS_HTML_VERSION};
use moss_core::contract::tokens::load_tokens;

#[test]
fn describe_payload_serializes_with_versions() {
    let tokens = load_tokens().expect("tokens");
    let payload = DescribePayload::new(&tokens);
    let json = serde_json::to_value(&payload).expect("serialize");

    assert_eq!(json["describe_schema_version"], DESCRIBE_SCHEMA_VERSION);
    assert_eq!(json["moss_html_version"], MOSS_HTML_VERSION);
    assert!(json["tokens"].is_object());
    assert!(json["components"].is_array());
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
