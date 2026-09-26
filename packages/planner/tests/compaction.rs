#[path = "../../runtime/tests/support/mod.rs"]
mod support;
use api::*;
use planner::*;
use runtime::HostBuilder;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use support::*;

#[derive(Default)]
struct SummaryModel {
    summaries: AtomicUsize,
    primary: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for SummaryModel {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> api::Result<ModelStream> {
        let events = if request
            .messages
            .first()
            .is_some_and(|m| m.text().starts_with("Summarize the earlier conversation."))
        {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            answer(&json!({"goal":"continue work","constraints":["do not repeat finished work"],"corrections":[],"decisions":[],"completed":[],"pending":["work remains"],"references":[]}).to_string())
        } else {
            let mut primary = self.primary.lock().unwrap();
            primary.push(request);
            if primary.len() == 1 {
                calls(&[("read-plan", PLAN_READ, json!({}))])
            } else {
                answer("continuing")
            }
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn actual_compaction_keeps_plan_state_separate_from_generated_summary_and_full_history() {
    let old_model = ScriptModel::new(vec![
        calls(&[(
            "old-plan",
            PLAN_UPDATE,
            json!({"revision":0,"goal":"keep this exact goal","steps":[{"id":"original","text":"keep exact step and id","status":"pending"}]}),
        )]),
        answer("plan recorded"),
    ]);
    let mut old_host = HostBuilder::new()
        .model(old_model)
        .plugin(Arc::new(PlannerPlugin::default()))
        .build()
        .await
        .unwrap();
    let old = old_host
        .engine()
        .execute(RunRequest::new("initial work"))
        .await
        .unwrap();
    old_host.shutdown().await.unwrap();
    let state = recover_history(&old.transcript).unwrap().unwrap();
    let state = state.enter_plan_mode(state.revision).unwrap();
    let state = state.submit(state.revision, "# Exact execution baseline\nConstraint 4567: never publish without separate release authorization.".into()).unwrap();
    let state = state.resume_execution(state.revision).unwrap();
    let mut history = old.transcript.clone();
    for i in 0..10 {
        history.push(Message::user(format!(
            "old user {i}: {}",
            "detail ".repeat(250)
        )));
        history.push(Message::Assistant {
            content: format!("old assistant {i}: {}", "observation ".repeat(170)),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
        });
    }
    history.push(Message::user("continue without replay"));
    let model = Arc::new(SummaryModel::default());
    let compactor = compaction::Compactor::new(compaction::CompactionConfig {
        max_summary_calls: 16,
        max_summary_bytes: 1024,
        ..Default::default()
    })
    .unwrap();
    let mut host = HostBuilder::new()
        .model(model.clone())
        .context_transform(Arc::new(compactor))
        .plugin(Arc::new(PlannerPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("unused");
    request.messages = history.clone();
    request.limits.max_context_bytes = 16 * 1024;
    bind_state(&mut request, &state).unwrap();
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert!(model.summaries.load(Ordering::SeqCst) > 0);
    assert!(report.transcript.starts_with(&history));
    assert_eq!(recover_history(&report.transcript).unwrap().unwrap(), state);
    for request in model.primary.lock().unwrap().iter() {
        assert!(request
            .messages
            .iter()
            .any(|m| m.text().contains("planner.state")
                && m.text().contains("keep exact step and id")
                && m.text().contains("Constraint 4567")));
        assert!(serde_json::to_vec(request).unwrap().len() <= 16 * 1024);
    }
    assert_eq!(
        report.task_usage.model_calls as usize,
        model.summaries.load(Ordering::SeqCst) + model.primary.lock().unwrap().len()
    );
    host.shutdown().await.unwrap();
}
