use api::*;
use compaction::{CompactionConfig, Compactor, TaskSummary};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

fn handoff(goal: &str) -> String {
    serde_json::to_string(&TaskSummary {
        goal: goal.into(),
        constraints: vec!["no legacy compatibility".into()],
        corrections: vec!["programming-only -> general reusable agent".into()],
        decisions: vec!["core / extensions / Web".into()],
        completed: vec!["write id=write-1 succeeded".into()],
        pending: vec!["deploy-2 Unknown: inspect effects, do not replay".into()],
        references: vec!["spill:sp_exact".into()],
    })
    .unwrap()
}
#[test]
fn malformed_handoffs_are_rejected_without_filling_missing_fields_or_clipping() {
    let good = handoff("continue safely");
    TaskSummary::parse(&good).unwrap();
    for invalid in [
        "free summary".into(),
        format!("```json\n{good}\n```"),
        good[..good.len() - 1].into(),
        good.replace("\"goal\":", "\"goal\":\"duplicate\",\"goal\":"),
        good.replace("\"constraints\":", "\"unknown\":"),
        good.replace("continue safely", " "),
    ] {
        assert_eq!(
            TaskSummary::parse(&invalid).unwrap_err().code,
            ErrorCode::ModelProtocol
        );
    }
}
struct Probe {
    calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    invalid: AtomicBool,
    long: bool,
}
#[async_trait]
impl ModelCaller for Probe {
    async fn complete(
        &self,
        request: ModelRequest,
        _: Option<Arc<dyn ModelSink>>,
    ) -> Result<ModelReply> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(matches!(request.messages[0], Message::System { .. }));
        assert!(request.messages[0].text().contains("old->current"));
        let input: serde_json::Value = serde_json::from_str(&request.messages[1].text()).unwrap();
        assert!(input["settled_history"].is_string());
        self.requests.lock().unwrap().push(request);
        Ok(ModelReply {
            content: if self.invalid.load(Ordering::SeqCst) {
                "invalid".into()
            } else {
                handoff(&if self.long {
                    "important-fact ".repeat(350)
                } else {
                    "retain all task facts".into()
                })
            },
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
            finish: FinishReason::Stop,
            usage: None,
        })
    }
}
fn context(model: Arc<Probe>, max: usize) -> RunContext {
    RunContext {
        run_id: "handoff".into(),
        task: TaskControl::default(),
        cancel: CancellationToken::new(),
        model,
        services: Services::default(),
        metadata: Arc::default(),
        model_options: ModelOptions::default(),
        limits: RunLimits {
            max_context_bytes: max,
            ..Default::default()
        },
        request_overhead_bytes: 100,
        request_tools: Arc::default(),
        context_sources: Arc::default(),
        model_context_window_tokens: None,
        tools_enabled: false,
        allowed_tools: None,
    }
}
fn assistant(content: &str) -> Message {
    Message::Assistant {
        content: content.into(),
        tool_calls: vec![],
        reasoning_content: None,
        provider_data: None,
    }
}
#[tokio::test]
async fn failed_update_keeps_the_previous_valid_state_and_the_complete_history() {
    let model = Arc::new(Probe {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        invalid: AtomicBool::new(false),
        long: false,
    });
    let ctx = context(model.clone(), 12000);
    let compactor = Compactor::new(CompactionConfig {
        recent_groups: 1,
        ..Default::default()
    })
    .unwrap();
    let original = vec![
        Message::user("old ".repeat(2600)),
        assistant("first answer"),
        Message::user("latest"),
    ];
    let projection = compactor.transform(&ctx, original.clone()).await.unwrap();
    let state = serde_json::to_vec(&compactor.state(&ctx.run_id).unwrap()).unwrap();
    model.invalid.store(true, Ordering::SeqCst);
    let mut more = original.clone();
    more.extend([assistant(&"new ".repeat(2600)), Message::user("newest")]);
    assert!(compactor.transform(&ctx, more).await.is_err());
    assert_eq!(
        serde_json::to_vec(&compactor.state(&ctx.run_id).unwrap()).unwrap(),
        state
    );
    assert_eq!(
        compactor
            .state(&ctx.run_id)
            .unwrap()
            .project(&original)
            .unwrap(),
        projection
    );
}
#[tokio::test]
async fn large_window_allows_a_handoff_larger_than_four_kib_without_changing_originals() {
    let model = Arc::new(Probe {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        invalid: AtomicBool::new(false),
        long: true,
    });
    let ctx = context(model, 32000);
    let compactor = Compactor::default();
    let original = vec![
        Message::user("old ".repeat(6600)),
        assistant("answer"),
        Message::user("latest"),
    ];
    let projection = compactor.transform(&ctx, original.clone()).await.unwrap();
    let state = compactor.state(&ctx.run_id).unwrap();
    assert!(state.summary.len() > 4096);
    assert!(
        serde_json::to_vec(&ctx.model_request(projection))
            .unwrap()
            .len()
            < ctx.limits.max_context_bytes
    );
    assert_eq!(original[0].text().len(), 26400);
}
struct CancelSummary {
    inner: Arc<Probe>,
    cancel: CancellationToken,
}
#[async_trait]
impl ModelCaller for CancelSummary {
    async fn complete(
        &self,
        request: ModelRequest,
        sink: Option<Arc<dyn ModelSink>>,
    ) -> Result<ModelReply> {
        let reply = self.inner.complete(request, sink).await?;
        self.cancel.cancel();
        Ok(reply)
    }
}
#[tokio::test]
async fn cancellation_and_active_cache_capacity_cannot_publish_or_evict_another_handoff() {
    let probe = Arc::new(Probe {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        invalid: AtomicBool::new(false),
        long: false,
    });
    let compactor = Compactor::new(CompactionConfig {
        max_cached_runs: 1,
        recent_groups: 1,
        ..Default::default()
    })
    .unwrap();
    let original = vec![
        Message::user("old ".repeat(2600)),
        assistant("first answer"),
        Message::user("latest"),
    ];
    let ctx = context(probe.clone(), 12000);
    compactor.transform(&ctx, original.clone()).await.unwrap();
    let saved = serde_json::to_vec(&compactor.state(&ctx.run_id).unwrap()).unwrap();
    let mut other = context(probe.clone(), 12000);
    other.run_id = "other".into();
    assert_eq!(
        compactor
            .transform(&other, original.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Limit
    );
    assert_eq!(
        serde_json::to_vec(&compactor.state(&ctx.run_id).unwrap()).unwrap(),
        saved
    );
    assert!(compactor.state("other").is_none());
    compactor.finish(&ctx.run_id).await.unwrap();
    other.model = Arc::new(CancelSummary {
        inner: probe,
        cancel: other.cancel.clone(),
    });
    assert_eq!(
        compactor
            .transform(&other, original)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert!(compactor.state("other").is_none());
}
#[tokio::test]
async fn indivisible_group_too_large_for_handoff_overhead_fails_before_any_summary_call() {
    let probe = Arc::new(Probe {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        invalid: AtomicBool::new(false),
        long: false,
    });
    let ctx = context(probe.clone(), 2400);
    let error = Compactor::default()
        .transform(
            &ctx,
            vec![Message::user("x".repeat(2200)), Message::user("latest")],
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Limit);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn private_protocol_groups_stay_verbatim_and_never_enter_summary_requests() {
    let model = Arc::new(Probe {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        invalid: AtomicBool::new(false),
        long: false,
    });
    let ctx = context(model.clone(), 12000);
    let private = Message::Assistant {
        content: "visible".into(),
        tool_calls: vec![],
        reasoning_content: Some("PRIVATE_THOUGHT".into()),
        provider_data: Some(ProviderData {
            namespace: "fixture".into(),
            value: serde_json::json!({"signature":"PRIVATE_SIGNATURE"}),
        }),
    };
    let original = vec![
        Message::user("old ".repeat(2500)),
        private.clone(),
        Message::user("latest"),
    ];
    let projected = Compactor::new(CompactionConfig {
        recent_groups: 1,
        ..Default::default()
    })
    .unwrap()
    .transform(&ctx, original)
    .await
    .unwrap();
    assert!(projected.contains(&private));
    for request in model.requests.lock().unwrap().iter() {
        let text = serde_json::to_string(request).unwrap();
        assert!(!text.contains("PRIVATE_THOUGHT"));
        assert!(!text.contains("PRIVATE_SIGNATURE"));
    }
}
