//! Real HTTP/Provider/Runtime/Spill/Read/Compaction composition, local model fixture only.
use super::*;
use api::*;
use axum::routing::post;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use std::time::Duration;

const TOKEN: &str = "local-context-governance-fixture-token-32";
fn source() -> String {
    "归档正文\n".repeat(180)
}

#[derive(Default)]
struct Probe {
    primary: AtomicUsize,
    summaries: AtomicUsize,
    executions: AtomicUsize,
    archive: Mutex<Option<String>>,
    read_text: Mutex<String>,
    cursor: Mutex<usize>,
    read_done: Mutex<bool>,
}

async fn mock_model(State(probe): State<Arc<Probe>>, Json(request): Json<Value>) -> Response {
    let messages = request["messages"].as_array().unwrap();
    let summary = messages
        .first()
        .and_then(|m| m["content"].as_str())
        .is_some_and(|text| text.starts_with("Summarize the earlier"));
    let (delta, reason) = if summary {
        probe.summaries.fetch_add(1, Ordering::SeqCst);
        (
            json!({"content":serde_json::to_string(&compaction::TaskSummary { goal: "Earlier tool calls completed. Continue reading the saved archive from the latest cursor.".into(), ..Default::default() }).unwrap()}),
            "stop",
        )
    } else {
        let round = probe.primary.fetch_add(1, Ordering::SeqCst);
        assert!(messages
            .iter()
            .any(|m| m["content"] == "Archive results, retrieve the original, and finish."));
        for message in messages {
            if message["role"] != "tool" {
                continue;
            }
            let result: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
            if let Some(uri) = result["artifact"]["uri"].as_str() {
                probe
                    .archive
                    .lock()
                    .unwrap()
                    .get_or_insert_with(|| uri.to_owned());
            }
        }
        if let Some(message) = messages.last().filter(|m| m["role"] == "tool") {
            let result: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
            if result["structured"]["unit"] == "utf8_bytes" {
                assert_eq!(result["status"], "success");
                probe
                    .read_text
                    .lock()
                    .unwrap()
                    .push_str(result["content"].as_str().unwrap());
                *probe.read_done.lock().unwrap() = result["structured"]["eof"].as_bool().unwrap();
                if let Some(next) = result["structured"]["next_offset"].as_u64() {
                    *probe.cursor.lock().unwrap() = next as usize;
                }
            }
        }
        if round < 6 {
            (
                json!({"tool_calls":[{"index":0,"id":format!("work-{round}"),"type":"function",
                "function":{"name":"large_work","arguments":"{}"}}]}),
                "tool_calls",
            )
        } else if *probe.read_done.lock().unwrap() {
            assert_eq!(*probe.read_text.lock().unwrap(), source());
            (
                json!({"content":"Archive retrieved and task completed."}),
                "stop",
            )
        } else {
            let args = json!({"path":probe.archive.lock().unwrap().clone().unwrap(),
                "offset":(*probe.cursor.lock().unwrap()).max(1),"limit":2048});
            (
                json!({"tool_calls":[{"index":0,"id":format!("read-{round}"),"type":"function",
                "function":{"name":"read","arguments":args.to_string()}}]}),
                "tool_calls",
            )
        }
    };
    let event = json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}]});
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

struct LargeWork(Arc<Probe>);
#[async_trait]
impl Tool for LargeWork {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "large_work".into(),
            description: "Local fixture side effect".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }
    async fn execute(&self, _: ToolContext, _: Value) -> api::Result<ToolOutput> {
        self.0.executions.fetch_add(1, Ordering::SeqCst);
        Ok(source().into())
    }
}

#[tokio::test]
async fn real_http_flow_archives_reads_compacts_and_finishes_without_replaying_work() {
    let probe = Arc::new(Probe::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/chat", listener.local_addr().unwrap());
    let model_stop = CancellationToken::new();
    let stop = model_stop.clone();
    let mock_router = Router::new()
        .route("/chat", post(mock_model))
        .with_state(probe.clone());
    let mock = tokio::spawn(async move {
        axum::serve(listener, mock_router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let config = SpillConfig::default();
    let store =
        Arc::new(LocalSpillStore::new(directory.path().join("spill"), config.clone()).unwrap());
    let spill_host = SpillHost { store, config };
    let mut tool_config = tools::ToolConfig::new(directory.path(), "unused-test-shell");
    tool_config
        .read_extensions
        .push(Arc::new(spill_host.read_extension()));
    let mut chat_config = providers::ChatConfig::new(endpoint, "local-fixture");
    chat_config.allow_http_loopback = true;
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(providers::ChatModel::new(chat_config).unwrap()))
        .tool(Arc::new(LargeWork(probe.clone())))
        .allow_side_effect_tool("large_work")
        .tool(Arc::new(tools::ReadTool::new(tool_config)))
        .plugin(Arc::new(spill_host.plugin()))
        .plugin(Arc::new(compaction::CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut app_config = application::ApplicationConfig::default();
    app_config.run_limits.max_context_bytes = 7000;
    app_config.run_limits.max_tool_result_bytes = 512;
    app_config.run_limits.max_steps = 48;
    app_config.task_limits.max_model_calls = 64;
    let app = application::AgentApplication::new(Arc::new(host.engine()), app_config).unwrap();
    let app_router = http_bridge::router(app.clone(), http_bridge::HttpConfig::new(TOKEN))
        .unwrap()
        .merge(router(spill_host, TOKEN.into(), None).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app_stop = CancellationToken::new();
    let stop = app_stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app_router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let started: Value = client.post(format!("{base}/v1/runs")).bearer_auth(TOKEN)
        .json(&json!({"request_id":"governance-flow","prompt":"Archive results, retrieve the original, and finish."}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let id = started["run_id"].as_str().unwrap();
    let stream = client
        .get(format!("{base}/v1/runs/{id}/events"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!stream.contains(&directory.path().to_string_lossy().to_string()));
    let result: Value = client
        .get(format!("{base}/v1/runs/{id}/result"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["outcome"]["status"], "completed", "{result}");
    assert_eq!(probe.executions.load(Ordering::SeqCst), 6);
    assert!(probe.summaries.load(Ordering::SeqCst) > 0);
    assert_eq!(*probe.read_text.lock().unwrap(), source());

    let uri = probe.archive.lock().unwrap().clone().unwrap();
    let archive_id = uri.strip_prefix("spill:").unwrap();
    let url = format!("{base}/api/spill/{id}/{archive_id}");
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let mut offset = 0;
    let mut full = String::new();
    loop {
        let page: spill::SpillPage = client
            .get(&url)
            .query(&[("offset", offset), ("limit", 256)])
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(page.text.len() <= 256);
        full.push_str(&page.text);
        offset = page.next_offset;
        if page.eof {
            break;
        }
    }
    assert_eq!(full, source());
    app.shutdown(Duration::from_secs(5)).await.unwrap();
    host.shutdown().await.unwrap();
    app_stop.cancel();
    model_stop.cancel();
    server.await.unwrap();
    mock.await.unwrap();
}
