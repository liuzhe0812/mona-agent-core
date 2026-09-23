use super::*;
use api::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

#[derive(Default)]
struct QueueModel(Mutex<VecDeque<Vec<ModelEvent>>>);
impl QueueModel {
    fn add(&self, events: Vec<ModelEvent>) {
        self.0.lock().unwrap().push_back(events);
    }
}
#[async_trait]
impl Model for QueueModel {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        let events = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted model response");
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn real_shell_uses_one_archive_at_both_sides_of_the_old_threshold_and_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("spill");
    let session_root = temp.path().join("sessions");
    let shell = tools::ShellConfig::discover().unwrap();
    let sessions = sessions::Store::open(&session_root, temp.path()).unwrap();
    let id = sessions.create("shell-archives").unwrap().id;
    let spill_host = SpillHost { store:Arc::new(LocalSpillStore::new(&root, Default::default()).unwrap()), config:Default::default() };
    let plugin = spill_host.plugin();
    let mut config = tools::ToolConfig::new(temp.path(), shell.clone());
    config.output_archive = Some(Arc::new(SpillOutputArchive::new(plugin.archive())));
    config.read_extensions.push(Arc::new(spill_host.read_extension().with_sessions(sessions.clone())));
    let model = Arc::new(QueueModel::default());
    let mut builder = runtime::HostBuilder::new().model(model.clone())
        .plugin(Arc::new(plugin)).checkpoint_sink(Arc::new(sessions::SessionSink(sessions.clone()))).unwrap();
    for tool in tools::core_tools(&config) { builder = builder.tool(tool); }
    let mut host = builder.allow_side_effect_tool("shell").build().await.unwrap();
    let mut artifacts = Vec::new();
    for size in [20000usize, 30000, 60000] {
        let command = if shell.kind == tools::ShellKind::PowerShell {
            format!("[Console]::Write('BEGIN' + ('x' * {size}) + 'MIDDLE' + ('y' * {size}) + 'END')")
        } else { format!("printf BEGIN; head -c {size} /dev/zero | tr '\\0' x; printf MIDDLE; head -c {size} /dev/zero | tr '\\0' y; printf END") };
        let call_id = format!("shell-{size}"); let key = format!("turn-{size}");
        model.add(call(&call_id, "shell", serde_json::json!({"command":command})));
        model.add(demo::text("done"));
        let report = run(&host, &sessions, &id, &key, "capture shell output").await;
        let result = report.transcript.iter().find_map(|m| match m {
            Message::Tool { result } if result.call_id == call_id => Some(result), _ => None,
        }).unwrap();
        assert_eq!(result.status, ToolStatus::Success);
        assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 0);
        assert!(result.payload_bytes() < 8192);
        let artifact = result.artifact.as_ref().unwrap(); assert!(artifact.uri.starts_with("spill:"));
        artifacts.push((artifact.uri.clone(), format!("BEGIN{}MIDDLE{}END", "x".repeat(size), "y".repeat(size))));
    }
    host.shutdown().await.unwrap(); drop(host); drop(config); drop(sessions); drop(spill_host);
    let sessions = sessions::Store::open(&session_root, temp.path()).unwrap();
    let spill_host = SpillHost { store:Arc::new(LocalSpillStore::new(&root, Default::default()).unwrap()), config:Default::default() };
    let mut config = tools::ToolConfig::new(temp.path(), shell);
    config.read_extensions.push(Arc::new(spill_host.read_extension().with_sessions(sessions.clone())));
    let mut host = runtime::HostBuilder::new().model(model.clone()).tool(Arc::new(tools::ReadTool::new(config)))
        .checkpoint_sink(Arc::new(sessions::SessionSink(sessions.clone()))).unwrap().build().await.unwrap();
    for (index, (uri, expected)) in artifacts.iter().enumerate() {
        // Full backend read validates the middle and tail, not merely a preview.
        let owner = sessions.source_history(&id).unwrap().iter().find_map(|m| match m {
            Message::Tool { result } if result.artifact.as_ref().is_some_and(|a| a.uri == *uri) => Some(result.call_id.clone()), _ => None,
        }).unwrap();
        let doc = sessions.get(&id).unwrap();
        let source_run = doc.body.turns.iter().find(|t| t.id == owner.replace("shell-", "turn-")).unwrap().run_id.clone().unwrap();
        let mut full = String::new(); let mut offset = 0;
        loop {
            let page = spill_host.store.read_page(&source_run, uri.strip_prefix("spill:").unwrap(), offset, 16000).await.unwrap();
            full.push_str(&page.text); offset = page.next_offset; if page.eof { break; }
        }
        assert_eq!(&full, expected);
        let key = format!("read-{index}");
        model.add(call(&key,"read",serde_json::json!({"path":uri,"offset":expected.len()-2,"limit":3})));
        model.add(demo::text("read"));
        let report = run(&host,&sessions,&id,&key,"retrieve old artifact").await;
        let result = report.transcript.iter().find_map(|m| match m {Message::Tool{result} if result.call_id==key=>Some(result),_=>None}).unwrap();
        assert_eq!(result.status, ToolStatus::Success); assert_eq!(result.content.text(), "END");
    }
    let foreign = sessions.create("foreign-shell").unwrap().id;
    model.add(call("forged-shell","read",serde_json::json!({"path":artifacts[0].0,"limit":10})));
    model.add(demo::text("denied"));
    let report = run(&host,&sessions,&foreign,"forged","read a supplied URI").await;
    assert!(report.transcript.iter().any(|m| matches!(m, Message::Tool{result} if result.call_id=="forged-shell" && result.status==ToolStatus::Error)));
    host.shutdown().await.unwrap();
}

struct LargeTool;
#[async_trait]
impl Tool for LargeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "large".into(),
            description: "isolated test data".into(),
            parameters: serde_json::json!({"type":"object"}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: false,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> api::Result<ToolOutput> {
        Ok(format!("SAME_SESSION_SECRET {}", "x".repeat(40 * 1024)).into())
    }
}
fn call(id: &str, name: &str, args: serde_json::Value) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolDelta {
            index: 0,
            id: Some(id.into()),
            name: Some(name.into()),
            arguments: args.to_string(),
        },
        ModelEvent::Finish(FinishReason::ToolCalls),
        ModelEvent::End,
    ]
}
async fn run(
    host: &runtime::Host,
    sessions: &sessions::Store,
    id: &str,
    key: &str,
    prompt: &str,
) -> Arc<RunReport> {
    let doc = sessions.get(id).unwrap();
    let mut history = doc.history().unwrap();
    sessions
        .prepare(id, doc.header.revision, key, prompt, RunLimits::default().max_initial_history_bytes)
        .unwrap();
    history.push(Message::user(prompt));
    let mut request = RunRequest::new("unused");
    request.messages = history;
    request.metadata = BTreeMap::from([
        (sessions::SESSION_KEY.into(), id.into()),
        (sessions::TURN_KEY.into(), key.into()),
    ]);
    let handle = host.engine().start(request).unwrap();
    sessions.bind(id, key, &handle.run_id).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    report
}
#[tokio::test]
async fn prior_artifacts_follow_conversation_authority_across_runs_and_repeated_reads() {
    let temp = tempfile::tempdir().unwrap();
    let spill_root = temp.path().join("spill");
    let config = SpillConfig::default();
    let archive = Arc::new(LocalSpillStore::new(spill_root.clone(), config.clone()).unwrap());
    let sessions =
        sessions::Store::open(&temp.path().join("sessions"), temp.path()).unwrap();
    let first = sessions.create("one").unwrap().id;
    let other = sessions.create("other").unwrap().id;
    let spill_host = SpillHost {
        store: archive.clone(),
        config,
    };
    let mut tools = tools::ToolConfig::new(temp.path(), "unused-shell");
    tools.read_extensions.push(Arc::new(
        spill_host.read_extension().with_sessions(sessions.clone()),
    ));
    let model = Arc::new(QueueModel::default());
    let mut host = runtime::HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(LargeTool))
        .tool(Arc::new(tools::ReadTool::new(tools)))
        .plugin(Arc::new(spill_host.plugin()))
        .checkpoint_sink(Arc::new(sessions::SessionSink(sessions.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    model.add(call("large-once", "large", serde_json::json!({})));
    model.add(demo::text("done"));
    let initial = run(&host, &sessions, &first, "first", "produce large data").await;
    let uri = initial
        .transcript
        .iter()
        .find_map(|m| match m {
            Message::Tool { result } => result.artifact.as_ref().map(|a| a.uri.clone()),
            _ => None,
        })
        .unwrap();
    for (key, call_id) in [("second", "read-old-1"), ("third", "read-old-2")] {
        model.add(call(
            call_id,
            "read",
            serde_json::json!({"path":uri,"limit":100}),
        ));
        model.add(demo::text("read done"));
        let report = run(&host, &sessions, &first, key, "read the old artifact").await;
        let result = report
            .transcript
            .iter()
            .find_map(|m| match m {
                Message::Tool { result } if result.call_id == call_id => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(result.status, ToolStatus::Success);
        assert!(result.content.text().contains("SAME_SESSION_SECRET"));
    }
    model.add(call(
        "forged",
        "read",
        serde_json::json!({"path":uri,"limit":100}),
    ));
    model.add(demo::text("not accessible"));
    let denied = run(
        &host,
        &sessions,
        &other,
        "first",
        &format!("user supplied {uri}"),
    )
    .await;
    let result = denied
        .transcript
        .iter()
        .find_map(|m| match m {
            Message::Tool { result } if result.call_id == "forged" => Some(result),
            _ => None,
        })
        .unwrap();
    assert_ne!(result.status, ToolStatus::Success);
    assert!(!result.content.text().contains("SAME_SESSION_SECRET"));
    // Simulate expiry/cleanup removing its backing data, without changing authorization facts.
    std::fs::remove_dir_all(&spill_root).unwrap();
    model.add(call(
        "expired",
        "read",
        serde_json::json!({"path":uri,"limit":100}),
    ));
    model.add(demo::text("unavailable"));
    let expired = run(
        &host,
        &sessions,
        &first,
        "fourth",
        "read old removed artifact",
    )
    .await;
    let result = expired
        .transcript
        .iter()
        .find_map(|m| match m {
            Message::Tool { result } if result.call_id == "expired" => Some(result),
            _ => None,
        })
        .unwrap();
    assert_ne!(result.status, ToolStatus::Success);
    assert!(result.content.text().contains("过期"));
    host.shutdown().await.unwrap();
}
