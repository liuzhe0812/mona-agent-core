use super::*;

fn model() -> ChatModel {
    let mut config = ChatConfig::new("https://example.invalid/v1/chat/completions", "model-a");
    config.message_replay_fields.insert("signed_blocks".into());
    config.tool_replay_fields.insert("signature".into());
    ChatModel::new(config).unwrap()
}
fn history(model: &ChatModel) -> Vec<Message> {
    let route = model.replay_route(&ModelOptions::default()).unwrap();
    let mut call = ToolCall::new("effect-1", "write", json!({"path":"done.txt"}));
    call.provider_data = Some(ProviderData {
        namespace: model.config.protocol_namespace.clone(),
        value: json!({"route":route,"fields":{"signature":"PRIVATE_TOOL_SIGNATURE"}}),
    });
    vec![
        Message::user("keep the completed action"),
        Message::Assistant {
            content: "done".into(),
            tool_calls: vec![call],
            reasoning_content: Some("PRIVATE_REASONING".into()),
            provider_data: Some(ProviderData {
                namespace: model.config.protocol_namespace.clone(),
                value: json!({"route":route,"fields":{"signed_blocks":["first","second"]}}),
            }),
        },
        Message::Tool {
            result: ToolResult::new("effect-1", ToolStatus::Success, "written"),
        },
    ]
}
#[test]
fn route_is_stable_after_reopen_and_key_rotation_but_not_model_endpoint_or_profile_changes() {
    let original = model();
    let saved = history(&original);
    let unchanged = saved.clone();
    for mutation in 0..5 {
        let mut target = model();
        match mutation {
            0 => target.config.api_key = Some("new-test-key".into()),
            1 => target.config.model = "model-b".into(),
            2 => {
                target.endpoint =
                    Url::parse("https://elsewhere.invalid/v1/chat/completions").unwrap()
            }
            3 => target.config.protocol_namespace = "another.provider".into(),
            _ => {
                target
                    .config
                    .extra_body
                    .insert("thinking".into(), json!(false));
            }
        }
        let result = target.validate_history(&saved, &ModelOptions::default());
        if mutation == 0 {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().code,
                ErrorCode::ModelHistoryIncompatible
            );
        }
    }
    let fresh = model();
    let request = ModelRequest {
        messages: saved.clone(),
        tools: vec![],
        max_output_tokens: 64,
        options: ModelOptions::default(),
    };
    let body = fresh.body(&request).unwrap();
    assert_eq!(
        body["messages"][1]["reasoning_content"],
        "PRIVATE_REASONING"
    );
    assert_eq!(
        body["messages"][1]["signed_blocks"],
        json!(["first", "second"])
    );
    assert_eq!(
        body["messages"][1]["tool_calls"][0]["signature"],
        "PRIVATE_TOOL_SIGNATURE"
    );
    assert!(body["messages"][1].get("route").is_none());
    assert_eq!(saved, unchanged);
}
#[test]
fn plain_history_can_switch_but_unscoped_reasoning_and_unscoped_signatures_cannot() {
    let mut target = model();
    target.config.model = "other".into();
    let plain = vec![
        Message::user("ordinary"),
        Message::Assistant {
            content: "answer".into(),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
        },
    ];
    target
        .validate_history(&plain, &ModelOptions::default())
        .unwrap();
    for private in [
        Message::Assistant {
            content: "".into(),
            tool_calls: vec![],
            reasoning_content: Some("old-private".into()),
            provider_data: None,
        },
        Message::Assistant {
            content: "".into(),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: Some(ProviderData {
                namespace: "chat_completions".into(),
                value: json!({"signed_blocks":["unscoped"]}),
            }),
        },
    ] {
        assert_eq!(
            target
                .validate_history(&[private], &ModelOptions::default())
                .unwrap_err()
                .code,
            ErrorCode::ModelHistoryIncompatible
        );
    }
    let mut compatible = model();
    compatible.config.allowed_models.insert("other".into());
    assert_eq!(
        compatible
            .validate_history(
                &history(&compatible),
                &ModelOptions {
                    model: Some("other".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::ModelHistoryIncompatible
    );
}
#[tokio::test]
async fn fragmented_reasoning_gets_one_scoped_snapshot_before_finish_without_exposing_origin_on_wire(
) {
    let model = model();
    let route = model.replay_route(&ModelOptions::default()).unwrap();
    let mut capture = ReplayCapture {
        namespace: model.config.protocol_namespace.clone(),
        route: route.clone(),
        reasoning_seen: false,
        message_fields: model.config.message_replay_fields.clone(),
        tool_fields: BTreeSet::new(),
        message: Map::new(),
        tools: Default::default(),
    };
    for fragment in ["PRIVATE_", "REASONING"] {
        let raw = json!({"choices":[{"delta":{"reasoning_content":fragment}}]}).to_string();
        assert!(
            capture.capture(&raw).unwrap().is_empty(),
            "do not emit quadratic snapshots for every token"
        );
    }
    let events = capture
        .capture(r#"{"choices":[{"delta":{"content":"answer"},"finish_reason":"stop"}]}"#)
        .unwrap();
    assert_eq!(events.len(), 1);
    let ModelEvent::ProviderData {
        target: ProtocolTarget::Assistant,
        data,
    } = &events[0]
    else {
        panic!("missing origin");
    };
    assert_eq!(data.value["route"], route);
    assert!(
        data.value["fields"].get("reasoning_content").is_none(),
        "origin metadata must not duplicate reasoning content"
    );
    let saved = vec![Message::Assistant {
        content: "answer".into(),
        tool_calls: vec![],
        reasoning_content: Some("PRIVATE_REASONING".into()),
        provider_data: Some(data.clone()),
    }];
    model
        .validate_history(&saved, &ModelOptions::default())
        .unwrap();
    let request = ModelRequest {
        messages: saved,
        tools: vec![],
        max_output_tokens: 64,
        options: ModelOptions::default(),
    };
    assert!(!serde_json::to_string(&model.body(&request).unwrap())
        .unwrap()
        .contains(&route));
}
