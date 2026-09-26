use api::*;
use mcp::{Config, Service};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
struct Invoke {
    tool: String,
    args: Value,
}
#[async_trait]
impl Model for Invoke {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let result = request.messages.iter().find_map(|m| {
            if let Message::Tool { result } = m {
                Some(result)
            } else {
                None
            }
        });
        let mut events = if let Some(result) = result {
            vec![
                ModelEvent::Text(format!("{:?}:{}", result.status, result.content.text())),
                ModelEvent::Finish(FinishReason::Stop),
            ]
        } else {
            vec![
                ModelEvent::ToolDelta {
                    index: 0,
                    id: Some("call-once".into()),
                    name: Some(self.tool.clone()),
                    arguments: self.args.to_string(),
                },
                ModelEvent::Finish(FinishReason::ToolCalls),
            ]
        };
        events.extend([
            ModelEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
            ModelEvent::End,
        ]);
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
fn config(log: &std::path::Path, extra: BTreeMap<String, String>) -> Config {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.mjs");
    let mut env = extra;
    env.insert("MCP_FIXTURE_LOG".into(), log.to_string_lossy().into_owned());
    env.insert("MCP_FIXTURE_KEY".into(), "test-only-key".into());
    serde_json::from_value(json!({"servers":{"fixture":{"enabled":true,"timeout_ms":5000,"transport":{"type":"stdio","command":"node","args":[path],"env":env}}}})).unwrap()
}
async fn run(service: &Arc<Service>, tool: &str, args: Value, authorize: bool) -> Arc<RunReport> {
    let mut builder = runtime::HostBuilder::new()
        .model(Arc::new(Invoke {
            tool: tool.into(),
            args,
        }))
        .plugin(Arc::new(service.plugin()));
    if authorize {
        builder = builder.allow_side_effect_tool(tool);
    }
    let mut host = builder.build().await.unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("MCP task"))
        .await
        .unwrap();
    host.shutdown().await.unwrap();
    report
}
#[tokio::test(flavor = "multi_thread")]
async fn stdio_handshake_pagination_names_schema_and_explicit_host_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.log");
    let service = Service::connect(config(&log, BTreeMap::new()), CancellationToken::new())
        .await
        .unwrap();
    let view = &service.views()[0];
    assert!(view.connected, "{:?}", view.error);
    assert_eq!(view.tools.len(), 6);
    assert!(
        !view.tools[0].read_only,
        "remote annotation must not grant host authority"
    );
    assert!(view.resources && view.has_instructions);
    let before = std::fs::read_to_string(&log).unwrap();
    let denied = run(
        &service,
        "mcp__fixture__echo",
        json!({"message":"NO_AUTH"}),
        false,
    )
    .await;
    assert!(denied.output.as_deref().unwrap_or("").contains("Denied"));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    let bad = run(&service, "mcp__fixture__echo", json!({"message":42}), true).await;
    assert!(bad.output.as_deref().unwrap_or("").contains("Error"));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    let good = run(
        &service,
        "mcp__fixture__echo",
        json!({"message":"HELLO"}),
        true,
    )
    .await;
    assert_eq!(good.status, RunStatus::Completed);
    assert!(good
        .output
        .as_ref()
        .unwrap()
        .contains("stdio:HELLO:host-secret=false:explicit-secret=true"));
    let failure = run(
        &service,
        "mcp__fixture__fail",
        json!({"message":"FAIL"}),
        true,
    )
    .await;
    assert!(failure
        .output
        .as_ref()
        .unwrap()
        .contains("Error:REMOTE_TOOL_ERROR"));
    let badoutput = run(
        &service,
        "mcp__fixture__bad_output",
        json!({"message":"FAIL"}),
        true,
    )
    .await;
    assert!(badoutput.output.as_ref().unwrap().contains("output schema"));
    let resource = run(
        &service,
        "mcp_read_resource",
        json!({"server":"fixture","uri":"fixture://stdio/readme"}),
        false,
    )
    .await;
    assert!(resource.output.as_ref().unwrap().contains("RESOURCE_stdio"));
    service.shutdown().await.unwrap();
    assert!(!service.views()[0].connected);
}
#[tokio::test(flavor = "multi_thread")]
async fn cancellation_stops_waiting_without_replaying_or_killing_other_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.log");
    let service = Service::connect(config(&log, BTreeMap::new()), CancellationToken::new())
        .await
        .unwrap();
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(Invoke {
            tool: "mcp__fixture__slow".into(),
            args: json!({"message":"block"}),
        }))
        .plugin(Arc::new(service.plugin()))
        .allow_side_effect_tool("mcp__fixture__slow")
        .build()
        .await
        .unwrap();
    let handle = host.engine().start(RunRequest::new("stop")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("\"call\":\"slow\"")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Cancelled);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("notifications/cancelled"));
    assert_eq!(text.matches("\"call\":\"slow\"").count(), 1);
    host.shutdown().await.unwrap();
    assert!(service.views()[0].connected);
    service.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread")]
async fn invalid_catalog_and_absent_executable_do_not_break_other_servers() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.log");
    let mut c = config(&log, BTreeMap::new());
    let mut bad = c.servers["fixture"].clone();
    if let mcp::TransportConfig::Stdio { env, .. } = &mut bad.transport {
        env.insert("MCP_FIXTURE_INVALID".into(), "1".into());
    }
    c.servers.insert("invalid".into(), bad);
    let mut missing = c.servers["fixture"].clone();
    if let mcp::TransportConfig::Stdio { command, .. } = &mut missing.transport {
        *command = "mona-nonexistent-mcp-executable-test".into();
    }
    c.servers.insert("missing".into(), missing);
    let service = Service::connect(c, CancellationToken::new()).await.unwrap();
    assert!(
        service
            .views()
            .iter()
            .find(|v| v.id == "fixture")
            .unwrap()
            .connected
    );
    let bad = service
        .views()
        .into_iter()
        .find(|v| v.id == "invalid")
        .unwrap();
    assert!(!bad.connected && bad.tools.is_empty() && bad.error.is_some());
    service.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread")]
async fn host_filters_preserve_none_and_timeout_does_not_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("calls.log");
    let mut c = config(&log, BTreeMap::new());
    let server = c.servers.get_mut("fixture").unwrap();
    server.allow_tools = Some(["echo".into(), "slow".into()].into());
    server.deny_tools.insert("echo".into());
    server.timeout_ms = 500;
    let service = Service::connect(c, CancellationToken::new()).await.unwrap();
    assert_eq!(service.views()[0].tools.len(), 1);
    let report = run(
        &service,
        "mcp__fixture__slow",
        json!({"message":"timeout"}),
        true,
    )
    .await;
    assert!(!report
        .output
        .as_deref()
        .unwrap_or_default()
        .contains("Success"));
    assert_eq!(
        std::fs::read_to_string(&log)
            .unwrap()
            .matches("\"call\":\"slow\"")
            .count(),
        1
    );
    service.shutdown().await.unwrap();
    let mut c = config(&log, BTreeMap::new());
    c.servers.get_mut("fixture").unwrap().allow_tools = Some(Default::default());
    let service = Service::connect(c, CancellationToken::new()).await.unwrap();
    assert!(service.views()[0].tools.is_empty());
    assert_eq!(service.tools().len(), 3);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancelled_startup_does_not_return_a_successful_empty_service() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        matches!(Service::connect(Config::default(),cancel).await,Err(e) if e.code==ErrorCode::Cancelled)
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn server_catalog_change_requires_explicit_new_assembly() {
    let tmp = tempfile::tempdir().unwrap();
    let service = Service::connect(
        config(&tmp.path().join("calls.log"), BTreeMap::new()),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(service.reconnect("fixture").await.unwrap().connected);
    run(
        &service,
        "mcp__fixture__change",
        json!({"message":"change"}),
        true,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(service.views()[0].needs_restart);
    service.shutdown().await.unwrap();
}
