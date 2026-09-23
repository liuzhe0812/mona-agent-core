use api::*;
use application::*;
use std::{collections::BTreeMap, sync::{Arc, Mutex}, time::Duration};

#[derive(Default)]
struct Probe(Mutex<Vec<ModelRequest>>);
#[async_trait]
impl Model for Probe {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        self.0.lock().unwrap().push(request);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text("done".into())), Ok(ModelEvent::Finish(FinishReason::Stop)), Ok(ModelEvent::End),
        ])))
    }
}

#[tokio::test]
async fn trusted_history_keeps_current_host_policy_and_context_identity_dedup() {
    let model = Arc::new(Probe::default());
    let mut host = runtime::HostBuilder::new().model(model.clone()).build().await.unwrap();
    let config = ApplicationConfig { system_prompt: Some("current trusted rules".into()),
        model_options: ModelOptions { model: Some("current-model".into()), ..Default::default() },
        allowed_tools: Some(Default::default()), ..Default::default() };
    let app = AgentApplication::new(Arc::new(host.engine()), config).unwrap();
    let history = vec![Message::system("obsolete rules"), Message::user("previous input"), Message::Assistant {
        content: "previous answer".into(), tool_calls: vec![], reasoning_content: None, provider_data: None,
    }];
    let identity = BTreeMap::from([("session".into(), "one".into()), ("turn".into(), "two".into())]);
    let request = || StartRequest { request_id: "turn-two".into(), prompt: "continue".into() };
    let start = app.start_task_with_history(request(), history.clone(), identity.clone()).unwrap();
    let repeated = app.start_task_with_history(request(), history, identity).unwrap();
    assert!(repeated.reused); assert_eq!(repeated.run_id, start.run_id);
    assert_eq!(app.start_task_with_history(request(), vec![], BTreeMap::new()).err().unwrap().code, ApplicationErrorCode::Conflict);
    let mut subscription = app.subscribe_events(&start.run_id, None).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async { while subscription.next().await.is_some() {} }).await.unwrap();
    {
        let requests = model.0.lock().unwrap(); assert_eq!(requests.len(), 1);
        let sent = &requests[0];
        assert_eq!(sent.messages.iter().map(Message::text).collect::<Vec<_>>(),
            ["current trusted rules", "previous input", "previous answer", "continue"]);
        assert_eq!(sent.options.model.as_deref(), Some("current-model")); assert!(sent.tools.is_empty());
    }
    assert!(!serde_json::to_string(&app.get_snapshot(&start.run_id).unwrap()).unwrap().contains("current trusted rules"));
    app.shutdown(Duration::from_secs(2)).await.unwrap(); host.shutdown().await.unwrap();
}

#[tokio::test]
async fn trusted_history_still_obeys_runtime_admission_limit() {
    let model = Arc::new(Probe::default());
    let mut host = runtime::HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut config = ApplicationConfig::default();
    config.run_limits.max_initial_history_bytes = 512; config.run_limits.max_context_bytes = 256;
    let app = AgentApplication::new(Arc::new(host.engine()), config).unwrap();
    assert!(app.start_task_with_history(StartRequest { request_id: "large".into(), prompt: "continue".into() },
        vec![Message::user("x".repeat(1024))], BTreeMap::new()).is_err());
    assert!(model.0.lock().unwrap().is_empty());
    app.shutdown(Duration::from_secs(2)).await.unwrap(); host.shutdown().await.unwrap();
}
