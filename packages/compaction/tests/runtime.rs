use api::*;
use compaction::CompactionPlugin;
use runtime::HostBuilder;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

#[derive(Default)]
struct ModelProbe {
    requests: Mutex<Vec<ModelRequest>>,
    overflow_once: AtomicBool,
    capacity: Option<u64>,
}

#[async_trait]
impl Model for ModelProbe {
    fn context_window_tokens(&self, _: &ModelOptions) -> Option<u64> { self.capacity }

    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        if let Some(capacity) = self.capacity {
            let estimate = serde_json::to_vec(&request).unwrap().len().div_ceil(3) as u64;
            assert!(estimate + request.max_output_tokens as u64 <= capacity,
                "request including summary must fit declared window: {estimate} + {} > {capacity}", request.max_output_tokens);
        }
        let summary = request
            .messages
            .first()
            .is_some_and(|message| message.text().starts_with("Summarize the earlier"));
        self.requests.lock().unwrap().push(request);
        if !summary && self.overflow_once.swap(false, Ordering::SeqCst) {
            return Err(AgentError::new(
                ErrorCode::ModelContextWindow,
                "injected context overflow",
            ));
        }
        let text = if summary {
            serde_json::to_string(&compaction::TaskSummary { goal: "old requirements preserved".into(), ..Default::default() }).unwrap()
        } else {
            "finished".into()
        };
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(text.into())),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}

struct Pass;
#[async_trait]
impl ContextTransform for Pass {
    async fn transform(&self, _: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
}

fn assistant(text: &str) -> Message {
    Message::Assistant {
        content: text.into(),
        tool_calls: vec![],
        reasoning_content: None,
        provider_data: None,
    }
}

#[tokio::test]
async fn plugin_compacts_the_projection_through_the_runtime_gateway() {
    let model = Arc::new(ModelProbe::default());
    let mut host = HostBuilder::new()
        .model(model.clone())
        .context_transform(Arc::new(Pass))
        .plugin(Arc::new(CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut original = vec![Message::system("rules")];
    for index in 0..7 {
        original.push(Message::user(format!("old-{index}-{}", "x".repeat(360))));
        original.push(assistant(&format!("answer-{index}-{}", "y".repeat(80))));
    }
    original.push(Message::user("current request"));
    original.push(assistant("recent context"));
    let mut request = RunRequest::new("unused");
    request.messages = original.clone();
    request.limits.max_context_bytes = 2600;
    assert!(serde_json::to_vec(&original).unwrap().len() > request.limits.max_context_bytes);
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.transcript[..original.len()], original);
    {
        let requests = model.requests.lock().unwrap();
        assert!(requests.len() >= 2);
        let primary = requests.last().unwrap();
        assert!(primary
            .messages
            .iter()
            .any(|message| message.text().contains("Earlier conversation summary")));
        assert!(primary
            .messages
            .iter()
            .any(|message| message.text() == "current request"));
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn confirmed_context_overflow_forces_one_smaller_retry() {
    let model = Arc::new(ModelProbe {
        requests: Mutex::new(vec![]),
        overflow_once: AtomicBool::new(true),
        capacity: None,
    });
    let mut host = HostBuilder::new()
        .model(model.clone())
        .plugin(Arc::new(CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut history = vec![Message::system("rules")];
    for index in 0..6 {
        history.push(Message::user(format!("old-{index}-{}", "x".repeat(300))));
        history.push(assistant(&format!("answer-{index}")));
    }
    history.push(Message::user("current request"));
    let mut request = RunRequest::new("unused");
    request.messages = history;
    request.limits.max_context_bytes = 16 * 1024;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    {
        let requests = model.requests.lock().unwrap();
        assert!(requests.iter().any(|request| request.messages.first()
            .is_some_and(|message| message.text().starts_with("Summarize the earlier"))));
        let primary = requests.iter().filter(|request| !request.messages.first()
            .is_some_and(|message| message.text().starts_with("Summarize the earlier"))).collect::<Vec<_>>();
        assert_eq!(primary.len(), 2);
        assert!(serde_json::to_vec(primary[1]).unwrap().len() < serde_json::to_vec(primary[0]).unwrap().len());
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn summary_calls_also_fit_the_known_model_window() {
    let model = Arc::new(ModelProbe { capacity: Some(900), ..Default::default() });
    let mut host = HostBuilder::new().model(model.clone())
        .plugin(Arc::new(CompactionPlugin::default())).build().await.unwrap();
    let mut request = RunRequest::new("unused");
    request.limits.max_context_bytes = 64 * 1024;
    request.limits.max_output_tokens = 100;
    request.messages = (0..6).flat_map(|i| [
        Message::user(format!("old-{i}-{}", "x".repeat(350))),
        assistant("prior result"),
    ]).collect();
    request.messages.push(Message::user("current"));
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert!(model.requests.lock().unwrap().len() >= 3, "summaries must be split into bounded requests");
    host.shutdown().await.unwrap();
}
