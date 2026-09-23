use crate::{native::Decoder, *};
use api::*;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn responses_initial_content_is_preserved_and_blocks_retry_before_later_deltas() {
    let mut decoder = crate::responses::ResponsesDecoder::new("responses".into(), "route".into());
    decoder
        .decode(json!({"type":"response.created","response":{"id":"resp"}}))
        .unwrap();
    let item = json!({"type":"message","id":"msg","role":"assistant","status":"in_progress",
        "content":[{"type":"output_text","text":"partial initial text","annotations":[]}]});
    let events = decoder
        .decode(json!({"type":"response.output_item.added","output_index":0,"item":item}))
        .unwrap();
    assert!(matches!(&events[..], [ModelEvent::Text(text)] if text == "partial initial text"));
    assert!(decoder.output_started());
    assert!(decoder
        .decode(json!({"type":"error","code":"server_error"}))
        .is_err());
    assert!(!decoder.done());
}

#[tokio::test]
async fn messages_input_overflow_is_classified_without_leaking_the_body() {
    let body = json!({"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 220000 tokens > 200000 maximum"}}).to_string();
    let (config, server) = fixture(Protocol::Messages, body, "400 Bad Request").await;
    let error = match create_model(config)
        .unwrap()
        .stream(request(vec![Message::user("go")]), CancellationToken::new())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("oversized prompt was accepted"),
    };
    assert_eq!(error.code, ErrorCode::ModelContextWindow);
    assert_eq!(error.http_status, Some(400));
    assert!(!error.model_output_started);
    assert!(!error.message.contains("220000"));
    server.await.unwrap();
    for message in [
        "quoted: prompt is too long",
        "prompt is too long: 1 tokens > 2 maximum",
        "prompt is too long: 220000 tokens > 200000 maximum SECRET",
    ] {
        let error = crate::chat::classify_provider_error(
            &json!({"type":"invalid_request_error","message":message}),
        );
        assert_eq!(error.code, ErrorCode::ModelRequest);
    }
    assert_eq!(
        crate::chat::classify_provider_error(
            &json!({"type":"rate_limit_error","message":"prompt is too long"})
        )
        .code,
        ErrorCode::ModelRateLimit
    );
}

fn config(protocol: Protocol) -> ProviderConfig {
    ProviderConfig::new(
        protocol,
        format!("https://example.invalid/v1{}", protocol.suffix()),
        "fixture",
    )
}
fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        messages,
        tools: vec![],
        max_output_tokens: 2048,
        options: ModelOptions::default(),
    }
}
fn image() -> ContentBlock {
    ContentBlock::Image {
        media_type: "image/png".into(),
        source: ImageSource::Base64 {
            data: "AQ==".into(),
        },
    }
}
fn assistant(text: &str, calls: Vec<ToolCall>, data: Option<ProviderData>) -> Message {
    Message::Assistant {
        content: text.into(),
        tool_calls: calls,
        reasoning_content: None,
        provider_data: data,
    }
}
fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: "lookup".into(),
        description: "lookup".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        concurrency: ToolConcurrency::ParallelSafe,
        side_effects: false,
    }
}
fn response_frames(private: bool, tool: bool) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
    ];
    let mut output = vec![];
    if private {
        let item = json!({"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"private summary"}]});
        events.push(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning","summary":[]}}));
        events.push(json!({"type":"response.reasoning_summary_text.delta","output_index":0,"item_id":"rs_1","summary_index":0,"delta":"private summary"}));
        events.push(json!({"type":"response.output_item.done","output_index":0,"item":item}));
        let mut item = item;
        item["encrypted_content"] = json!("opaque-signed-state");
        output.push(item);
    }
    let i = output.len();
    let item = if tool {
        let item = json!({"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{\"path\":\"你好\"}","status":"completed"});
        events.extend([
            json!({"type":"response.output_item.added","output_index":i,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"","status":"in_progress"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":i,"item_id":"fc_1","delta":"{\"path\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":i,"item_id":"fc_1","delta":"\"你好\"}"}),
            json!({"type":"response.function_call_arguments.done","output_index":i,"item_id":"fc_1","arguments":item["arguments"]}),
        ]);
        item
    } else {
        let item = json!({"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"你好","annotations":[]}],"status":"completed"});
        events.extend([
            json!({"type":"response.output_item.added","output_index":i,"item":{"type":"message","id":"msg_1","role":"assistant","content":[],"status":"in_progress"}}),
            json!({"type":"response.content_part.added","output_index":i,"item_id":"msg_1","content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.delta","output_index":i,"item_id":"msg_1","content_index":0,"delta":"你"}),
            json!({"type":"response.output_text.delta","output_index":i,"item_id":"msg_1","content_index":0,"delta":"好"}),
            json!({"type":"response.output_text.done","output_index":i,"item_id":"msg_1","content_index":0,"text":"你好"}),
        ]);
        item
    };
    events.push(json!({"type":"response.output_item.done","output_index":i,"item":item}));
    output.push(item);
    events.push(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output,"usage":{"input_tokens":12,"output_tokens":8,"input_tokens_details":{"cached_tokens":3}}}}));
    events
}
fn message_frames(private: bool, tool: bool) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"fixture","stop_reason":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":4,"cache_read_input_tokens":3,"output_tokens":1}}}),
    ];
    let mut i = 0;
    if private {
        events.extend([
            json!({"type":"content_block_start","index":i,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"thinking_delta","thinking":"hidden reasoning"}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"signature_delta","signature":"signed-"}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"signature_delta","signature":"value"}}),
            json!({"type":"content_block_stop","index":i}),
        ]);
        i += 1;
    }
    if tool {
        events.extend([
            json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":"\"你好\"}"}}),
            json!({"type":"content_block_stop","index":i}),
        ]);
    } else {
        events.extend([
            json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":"你"}}),
            json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":"好"}}),
            json!({"type":"content_block_stop","index":i}),
        ]);
    }
    events.extend([json!({"type":"message_delta","delta":{"stop_reason":if tool{"tool_use"}else{"end_turn"},"stop_sequence":null},"usage":{"output_tokens":8}}),json!({"type":"message_stop"})]);
    events
}
fn decode(protocol: Protocol, frames: Vec<Value>, route: String) -> Vec<ModelEvent> {
    let mut decoder: Box<dyn Decoder> = match protocol {
        Protocol::Responses => Box::new(crate::responses::ResponsesDecoder::new(
            protocol.name().into(),
            route,
        )),
        Protocol::Messages => Box::new(crate::messages::MessagesDecoder::new(
            protocol.name().into(),
            route,
        )),
        _ => unreachable!(),
    };
    let mut output = vec![];
    for frame in frames {
        output.extend(decoder.decode(frame).unwrap());
    }
    assert!(decoder.done());
    output
}
fn reply(events: &[ModelEvent]) -> Message {
    let mut text = String::new();
    let mut calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    let mut data = None;
    for event in events {
        match event {
            ModelEvent::Text(s) => text.push_str(s),
            ModelEvent::ToolDelta {
                index,
                id,
                name,
                arguments,
            } => {
                let entry = calls.entry(*index).or_default();
                if let Some(id) = id {
                    entry.0 = id.clone();
                }
                if let Some(name) = name {
                    entry.1 = name.clone();
                }
                entry.2.push_str(arguments);
            }
            ModelEvent::ProviderData {
                target: ProtocolTarget::Assistant,
                data: d,
            } => data = Some(d.clone()),
            _ => {}
        }
    }
    assistant(
        &text,
        calls
            .into_values()
            .map(|(id, name, args)| ToolCall::new(id, name, serde_json::from_str(&args).unwrap()))
            .collect(),
        data,
    )
}
#[test]
fn native_request_mapping_preserves_tools_results_media_and_local_ownership() {
    let calls = vec![
        ToolCall::new("call_1", "lookup", json!({"path":"one"})),
        ToolCall::new("call_2", "lookup", json!({"path":"two"})),
    ];
    let mut result =
        ToolResult::from_output("call_1", ToolOutput::new(Content::Blocks(vec![image()])));
    result.structured = Some(json!({"meaning":7}));
    let mut req = request(vec![
        Message::system("rules"),
        Message::user(Content::Blocks(vec![
            ContentBlock::Text {
                text: "look".into(),
            },
            image(),
        ])),
        assistant("", calls, None),
        Message::Tool { result },
        Message::Tool {
            result: ToolResult::new("call_2", ToolStatus::Error, "not found"),
        },
    ]);
    req.tools = vec![tool_spec()];
    let responses = ResponsesModel::new(config(Protocol::Responses))
        .unwrap()
        .body(&req)
        .unwrap();
    assert_eq!(responses["store"], false);
    assert!(responses.get("previous_response_id").is_none());
    assert!(responses.get("conversation").is_none());
    assert_eq!(responses["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(responses["tools"][0]["strict"], false);
    assert_eq!(responses["input"][1]["content"][1]["type"], "input_image");
    assert_eq!(responses["input"][4]["output"][1]["type"], "input_image");
    let messages = MessagesModel::new(config(Protocol::Messages))
        .unwrap()
        .body(&req)
        .unwrap();
    assert_eq!(messages["system"][0]["text"], "rules");
    assert_eq!(messages["messages"].as_array().unwrap().len(), 3);
    assert_eq!(messages["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(messages["messages"][2]["content"][1]["is_error"], true);
    assert_eq!(
        messages["messages"][2]["content"][0]["content"][1]["source"]["type"],
        "base64"
    );
    assert!(messages.get("tools").is_some());
}
#[test]
fn private_state_preserves_order_signatures_and_exact_route_but_never_becomes_visible_reasoning() {
    for protocol in [Protocol::Responses, Protocol::Messages] {
        let http = crate::native::Http::new(config(protocol), protocol).unwrap();
        let route = http.route(&ModelOptions::default()).unwrap();
        let frames = if protocol == Protocol::Responses {
            response_frames(true, true)
        } else {
            message_frames(true, true)
        };
        let events = decode(protocol, frames, route);
        assert!(!events.iter().any(|e| matches!(e, ModelEvent::Reasoning(_))));
        assert!(matches!(
            events[events.len() - 2],
            ModelEvent::Finish(FinishReason::ToolCalls)
        ));
        let history = vec![
            Message::user("do it"),
            reply(&events),
            Message::Tool {
                result: ToolResult::new("call_1", ToolStatus::Success, "done"),
            },
        ];
        let req = request(history.clone());
        let body = if protocol == Protocol::Responses {
            ResponsesModel::new(config(protocol))
                .unwrap()
                .body(&req)
                .unwrap()
        } else {
            MessagesModel::new(config(protocol))
                .unwrap()
                .body(&req)
                .unwrap()
        };
        let wire = body.to_string();
        assert!(!wire.contains("\"route\""));
        assert!(!wire.contains("\"namespace\""));
        assert!(wire.contains(if protocol == Protocol::Responses {
            "opaque-signed-state"
        } else {
            "signed-value"
        }));
        let mut different = config(protocol);
        different.model = "another".into();
        let adapter = create_model(different).unwrap();
        assert_eq!(
            adapter
                .validate_history(&history, &ModelOptions::default())
                .unwrap_err()
                .code,
            ErrorCode::ModelHistoryIncompatible
        );
        let other = if protocol == Protocol::Messages {
            Protocol::Responses
        } else {
            Protocol::Messages
        };
        assert_eq!(
            create_model(config(other))
                .unwrap()
                .validate_history(&history, &ModelOptions::default())
                .unwrap_err()
                .code,
            ErrorCode::ModelHistoryIncompatible
        );
        let usage = events
            .iter()
            .filter_map(|e| {
                if let ModelEvent::Usage(u) = e {
                    Some(u)
                } else {
                    None
                }
            })
            .last()
            .unwrap();
        assert_eq!(
            usage.input_tokens,
            if protocol == Protocol::Messages {
                17
            } else {
                12
            }
        );
        assert_eq!(usage.output_tokens, 8);
    }
}
#[test]
fn declared_capabilities_options_and_remote_features_fail_explicitly() {
    for protocol in [Protocol::Responses, Protocol::Messages] {
        for key in [
            "tools",
            "store",
            "background",
            "conversation",
            "previous_response_id",
            "api_key",
            "headers",
        ] {
            let mut c = config(protocol);
            c.extra_body.insert(key.into(), json!(true));
            assert!(create_model(c).is_err());
        }
        let mut c = config(protocol);
        c.capabilities.images = Some(false);
        assert_eq!(
            create_model(c)
                .unwrap()
                .validate_history(
                    &[Message::user(Content::Blocks(vec![image()]))],
                    &ModelOptions::default()
                )
                .unwrap_err()
                .code,
            ErrorCode::Unsupported
        );
        let mut c = config(protocol);
        c.capabilities.tools = Some(false);
        let mut req = request(vec![Message::user("go")]);
        req.tools = vec![tool_spec()];
        let error = if protocol == Protocol::Responses {
            ResponsesModel::new(c).unwrap().body(&req).unwrap_err()
        } else {
            MessagesModel::new(c).unwrap().body(&req).unwrap_err()
        };
        assert_eq!(error.code, ErrorCode::Unsupported);
    }
    let mut req = request(vec![Message::user("go")]);
    req.options.stop = Some(vec!["END".into()]);
    assert_eq!(
        ResponsesModel::new(config(Protocol::Responses))
            .unwrap()
            .body(&req)
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        MessagesModel::new(config(Protocol::Messages))
            .unwrap()
            .body(&req)
            .unwrap()["stop_sequences"],
        json!(["END"])
    );
}
#[test]
fn protocol_completion_and_signed_replay_are_required() {
    let mut frames = response_frames(true, true);
    frames.last_mut().unwrap()["response"]["output"][0]
        .as_object_mut()
        .unwrap()
        .remove("encrypted_content");
    let mut d = crate::responses::ResponsesDecoder::new("responses".into(), "r".into());
    let mut failed = false;
    for frame in frames {
        if d.decode(frame).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed && !d.done());
    let mut d = crate::messages::MessagesDecoder::new("messages".into(), "r".into());
    assert!(d.decode(json!({"type":"message_stop"})).is_err());
    let frames = message_frames(false, false);
    let mut d = crate::messages::MessagesDecoder::new("messages".into(), "r".into());
    let mut start = frames[0].clone();
    start["message"]["usage"]["cache_creation_input_tokens"] = json!("bad");
    assert!(d.decode(start).is_err());
    let mut d = crate::responses::ResponsesDecoder::new("responses".into(), "r".into());
    let mut frames = response_frames(false, true);
    let last = frames.last_mut().unwrap();
    last["response"]["output"][0]["arguments"] = json!("{\"path\":\"changed\"}");
    for frame in frames {
        if let Err(e) = d.decode(frame) {
            assert_eq!(e.code, ErrorCode::ModelProtocol);
            assert!(d.output_started());
            return;
        }
    }
    panic!("contradicting terminal tool result accepted");
}
async fn fixture(
    protocol: Protocol,
    body: String,
    status: &str,
) -> (ProviderConfig, tokio::task::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut c = ProviderConfig::new(
        protocol,
        format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            protocol.suffix()
        ),
        "fixture",
    );
    c.api_key = Some("test-only-key".into());
    c.allow_http_loopback = true;
    let status = status.to_owned();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let (end, len) = loop {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, len);
            }
        };
        while bytes.len() < end + len {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let value = serde_json::from_slice(&bytes[end..end + len]).unwrap();
        let header=format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\nRetry-After: 2\r\n\r\n",body.len());
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(body.as_bytes()).await.unwrap();
        (headers, value)
    });
    (c, handle)
}
fn sse(frames: &[Value]) -> String {
    frames
        .iter()
        .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
        .collect()
}
#[tokio::test]
async fn actual_http_uses_protocol_auth_and_requires_native_terminal_not_tcp_eof() {
    for protocol in [Protocol::Responses, Protocol::Messages] {
        let frames = if protocol == Protocol::Responses {
            response_frames(false, false)
        } else {
            message_frames(false, false)
        };
        let (c, server) = fixture(protocol, sse(&frames), "200 OK").await;
        let events = create_model(c)
            .unwrap()
            .stream(request(vec![Message::user("go")]), CancellationToken::new())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(|e| e.is_ok()));
        assert!(matches!(events.last(), Some(Ok(ModelEvent::End))));
        let (headers, body) = server.await.unwrap();
        let headers = headers.to_ascii_lowercase();
        if protocol == Protocol::Messages {
            assert!(headers.contains("x-api-key: test-only-key"));
            assert!(headers.contains("anthropic-version: 2023-06-01"));
            assert!(!headers.contains("authorization:"));
        } else {
            assert!(headers.contains("authorization: bearer test-only-key"));
            assert_eq!(body["store"], false);
        }
        let (c, server) = fixture(protocol, sse(&frames[..frames.len() - 1]), "200 OK").await;
        let events = create_model(c)
            .unwrap()
            .stream(request(vec![Message::user("go")]), CancellationToken::new())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        let e = events.last().unwrap().as_ref().err().unwrap();
        assert_eq!(e.code, ErrorCode::ModelTransport);
        assert!(e.model_output_started);
        assert!(!events.iter().any(|e| matches!(e, Ok(ModelEvent::End))));
        server.await.unwrap();
    }
}
#[tokio::test]
async fn later_error_in_same_chunk_does_not_erase_content_or_allow_automatic_retry() {
    for protocol in [Protocol::Responses, Protocol::Messages] {
        let mut frames = if protocol == Protocol::Responses {
            response_frames(false, false)
        } else {
            message_frames(false, false)
        };
        frames.truncate(frames.len() - 1);
        frames.push(
            json!({"type":"error","error":{"type":"overloaded_error"},"code":"server_error"}),
        );
        let (c, server) = fixture(protocol, sse(&frames), "200 OK").await;
        let events = create_model(c)
            .unwrap()
            .stream(request(vec![Message::user("go")]), CancellationToken::new())
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(events
            .iter()
            .any(|e| matches!(e,Ok(ModelEvent::Text(s)) if !s.is_empty())));
        let error = events.last().unwrap().as_ref().err().unwrap();
        assert!(error.model_output_started);
        assert_eq!(error.code, ErrorCode::ModelServer);
        server.await.unwrap();
        let (c, server) = fixture(protocol, "{}".into(), "429 Too Many Requests").await;
        let error = match create_model(c)
            .unwrap()
            .stream(request(vec![Message::user("go")]), CancellationToken::new())
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("429 accepted"),
        };
        assert_eq!(error.code, ErrorCode::ModelRateLimit);
        assert_eq!(error.retry_after_ms, Some(2000));
        assert!(!error.message.contains("test-only-key"));
        server.await.unwrap();
    }
}
