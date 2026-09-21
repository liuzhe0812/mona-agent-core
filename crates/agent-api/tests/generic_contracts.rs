use agent_api::*;
use serde_json::{json, Value};

fn image(data: &str) -> ContentBlock {
    ContentBlock::Image { media_type: "image/png".into(), source: ImageSource::Base64 { data: data.into() } }
}

#[test]
fn legacy_text_stays_a_json_string() {
    assert_eq!(serde_json::to_value(Content::from("hello")).unwrap(), json!("hello"));
    let message: Message = serde_json::from_value(json!({"role":"user","content":"hello"})).unwrap();
    assert_eq!(message.text(), "hello");
}

#[test]
fn multimodal_blocks_preserve_order_and_type() {
    let content = Content::from(vec![ContentBlock::Text { text: "before".into() }, image("AQ=="),
        ContentBlock::Text { text: "after".into() }]);
    content.validate().unwrap();
    let encoded = serde_json::to_value(&content).unwrap();
    assert_eq!(encoded[1]["type"], "image");
    assert_eq!(serde_json::from_value::<Content>(encoded).unwrap(), content);
    assert_eq!(content.text(), "before\nafter");
    assert!(content.has_media());
}

#[test]
fn base64_validation_is_strict_but_does_not_claim_image_decoding() {
    for value in ["AQ==", "AQI=", "AQID", "AAAA"] {
        Content::from(vec![image(value)]).validate().unwrap();
    }
    for value in ["", "A", "AQ", "AR==", "AQJ=", "A===", "A Q=", "====", "AQ==\n"] {
        assert!(Content::from(vec![image(value)]).validate().is_err(), "accepted {value:?}");
    }
}

#[test]
fn resource_reference_is_a_descriptor_not_file_contents() {
    let c = Content::from(vec![ContentBlock::Resource {
        reference: ArtifactRef { uri: "asset://document/123".into(), bytes: 4096 },
        media_type: "application/pdf".into(), name: Some("example.pdf".into()),
    }]);
    c.validate().unwrap();
    assert_eq!(c.text(), "");
    assert!(!c.preview().contains("123"));
}

#[test]
fn content_limits_and_unrecognized_image_media_are_rejected() {
    assert!(Content::Blocks(vec![]).validate().is_err());
    assert!(Content::Blocks(vec![image("AQ=="); 129]).validate().is_err());
    let c = Content::Blocks(vec![ContentBlock::Image { media_type: "image/svg+xml".into(),
        source: ImageSource::Base64 { data: "AQ==".into() } }]);
    assert_eq!(c.validate().unwrap_err().code, ErrorCode::Unsupported);
}

#[test]
fn known_tool_error_retains_structured_payload() {
    let mut output = ToolOutput::error("conflict");
    output.structured = Some(json!({"expected_revision":4,"actual_revision":5}));
    let result = ToolResult::from_output("call-1", output);
    assert_eq!(result.status, ToolStatus::Error);
    assert_eq!(result.structured.as_ref().unwrap()["actual_revision"], 5);
    assert_eq!(result.call_id, "call-1");
}

#[test]
fn public_tool_result_does_not_copy_media_or_structured_secrets() {
    let mut output = ToolOutput::new(Content::from(vec![ContentBlock::Text { text: "screenshot".into() },
        ContentBlock::Image { media_type: "image/png".into(), source: ImageSource::Url { url: "https://example.invalid/private-token".into() } }]));
    output.structured = Some(json!({"private":"not-for-ui"}));
    let result = ToolResult::from_output("one", output);
    let ui = UiToolResult::from_result(&result);
    let encoded = serde_json::to_value(&ui).unwrap();
    assert!(encoded["content"].is_string());
    assert!(ui.truncated);
    assert!(!encoded.to_string().contains("private-token"));
    assert!(!encoded.to_string().contains("not-for-ui"));
    assert_eq!(STREAM_VERSION, 2);
}

#[test]
fn opaque_provider_data_roundtrips_without_reordering_arrays() {
    let data = ProviderData { namespace: "test-v1".into(),
        value: json!({"blocks":[{"type":"signed","signature":"s1"},{"type":"redacted","payload":"x"}]}) };
    data.validate().unwrap();
    let copy: ProviderData = serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
    assert_eq!(copy, data);
    assert!(!format!("{data:?}").contains("s1"));
}

#[test]
fn invalid_or_excessive_opaque_data_is_rejected() {
    assert!(ProviderData { namespace: "provider space".into(), value: Value::Null }.validate().is_err());
    assert!(ProviderData { namespace: "p".into(), value: json!("x".repeat(MAX_PROVIDER_DATA_BYTES)) }.validate().is_err());
}

#[test]
fn options_inherit_without_merging_opaque_objects() {
    let defaults = ModelOptions { model: Some("model-a".into()), temperature: Some(0.2),
        stop: Some(vec!["END".into()]), provider_options: Some(ProviderData { namespace: "p".into(), value: json!({"a":1}) }),
        ..Default::default() };
    let override_ = ModelOptions { stop: Some(vec![]), provider_options: Some(ProviderData { namespace: "p".into(), value: json!({"b":2}) }), ..Default::default() };
    let effective = override_.inherit(&defaults);
    assert_eq!(effective.model.as_deref(), Some("model-a"));
    assert_eq!(effective.temperature, Some(0.2));
    assert_eq!(effective.stop, Some(vec![]));
    assert_eq!(effective.provider_options.unwrap().value, json!({"b":2}));
}

#[test]
fn invalid_options_and_credential_fields_are_not_accepted() {
    assert!(ModelOptions { temperature: Some(f64::NAN), ..Default::default() }.validate().is_err());
    assert!(ModelOptions { top_p: Some(0.0), ..Default::default() }.validate().is_err());
    assert!(serde_json::from_value::<ModelOptions>(json!({"api_key":"secret"})).is_err());
    assert!(serde_json::from_value::<ModelOptions>(json!({"max_output_tokens":100000})).is_err());
}

#[test]
fn artifact_bytes_are_part_of_result_budget() {
    let mut result = ToolResult::new("one", ToolStatus::Success, "ok");
    result.artifact = Some(ArtifactRef { uri: "asset://a".into(), bytes: 1024 });
    result.validate().unwrap();
    assert!(result.payload_bytes() > result.content.byte_len());
    result.artifact.as_mut().unwrap().uri = "x".repeat(4097);
    assert!(result.validate().is_err());
}

#[test]
fn rust_api_version_and_ui_protocol_are_independent() {
    assert_eq!(API_VERSION, 3);
    assert_eq!(STREAM_VERSION, 2);
    assert_eq!(CHECKPOINT_VERSION, 1);
}
