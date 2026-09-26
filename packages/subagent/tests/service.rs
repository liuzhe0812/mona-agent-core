use api::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use subagent::{Config, ContextMode, Driver, Launch, Role, Service, Spawn};
use tokio::sync::Notify;

#[derive(Default)]
struct LocalDriver(Mutex<Option<Weak<dyn AgentRuntime>>>);
impl Driver for LocalDriver {
    fn prepare(&self, parent: &RunContext, _role: &Role) -> Result<Launch> {
        Ok(Launch {
            runtime: self
                .0
                .lock()
                .unwrap()
                .as_ref()
                .and_then(Weak::upgrade)
                .unwrap(),
            model_options: parent.model_options.clone(),
            model: None,
            metadata: BTreeMap::new(),
        })
    }
}
struct ModelProbe {
    root: Notify,
    requests: Mutex<Vec<ModelRequest>>,
    writes: Arc<AtomicUsize>,
}
#[async_trait]
impl Model for ModelProbe {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        let task = request
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User { content } => Some(content.text().into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        self.requests.lock().unwrap().push(request.clone());
        if task == "ROOT" {
            tokio::select! {_=self.root.notified()=>{},_=cancel.cancelled()=>return Err(AgentError::new(ErrorCode::Cancelled,"root stopped"))};
        }
        if task == "BLOCK" {
            cancel.cancelled().await;
            return Err(AgentError::new(ErrorCode::Cancelled, "child stopped"));
        }
        let wrote = request
            .messages
            .iter()
            .any(|m| matches!(m,Message::Tool{result} if result.call_id=="write-once"));
        let events = if task == "WRITE" && !wrote {
            vec![
                ModelEvent::ToolDelta {
                    index: 0,
                    id: Some("write-once".into()),
                    name: Some("write_probe".into()),
                    arguments: "{}".into(),
                },
                ModelEvent::Finish(FinishReason::ToolCalls),
            ]
        } else {
            let secret = request
                .messages
                .iter()
                .any(|m| m.text().contains("ROOT_SECRET"));
            let mail = request
                .messages
                .iter()
                .any(|m| m.text().contains("ADDITIONAL_FACT"));
            let delegated = request.tools.iter().any(|t| t.name == "spawn_agent");
            vec![
                ModelEvent::Text(format!(
                    "task={task};secret={secret};mail={mail};delegation={delegated};writes={}",
                    self.writes.load(Ordering::SeqCst)
                )),
                ModelEvent::Finish(FinishReason::Stop),
            ]
        };
        let mut events = events;
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
struct Writer(Arc<AtomicUsize>);
#[async_trait]
impl Tool for Writer {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_probe".into(),
            description: "A counted side effect".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: true,
        }
    }
    async fn execute(&self, _ctx: ToolContext, _args: serde_json::Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new("written"))
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<sessions::Store>,
    service: Arc<Service>,
    host: runtime::Host,
    _runtime: Arc<dyn AgentRuntime>,
    model: Arc<ModelProbe>,
    root: RunHandle,
    id: String,
    task: TaskControl,
}
impl Fixture {
    async fn new(config: Config, max_calls: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = sessions::Store::open(&dir.path().join("sessions"), dir.path()).unwrap();
        let service = Service::new(store.clone(), config).unwrap();
        let driver = Arc::new(LocalDriver::default());
        let model = Arc::new(ModelProbe {
            root: Notify::new(),
            requests: Mutex::new(vec![]),
            writes: Arc::new(AtomicUsize::new(0)),
        });
        let binding = service.binding(driver.clone());
        let mut builder = runtime::HostBuilder::new()
            .model(model.clone())
            .checkpoint_sink(Arc::new(sessions::SessionSink(store.clone())))
            .unwrap()
            .plugin(Arc::new(binding.plugin()))
            .tool(Arc::new(Writer(model.writes.clone())))
            .allow_side_effect_tool("write_probe");
        for name in [
            "spawn_agent",
            "send_message",
            "followup_agent",
            "interrupt_agent",
        ] {
            builder = builder.allow_side_effect_tool(name);
        }
        let host = builder.build().await.unwrap();
        let runtime = sessions::runtime(Arc::new(host.engine()), store.clone());
        *driver.0.lock().unwrap() = Some(Arc::downgrade(&runtime));
        let h = store.create("parent").unwrap();
        let h = store
            .initialize_history(
                &h.id,
                h.revision,
                vec![Message::user("ROOT_SECRET")],
                BTreeMap::new(),
            )
            .unwrap();
        let history = match store
            .prepare(&h.id, h.revision, "main", "ROOT", 100000)
            .unwrap()
        {
            sessions::Prepared::New { history } => history,
            _ => panic!(),
        };
        let task = TaskControl::new(TaskLimits {
            max_model_calls: max_calls,
            wall_time: Duration::from_secs(30),
            ..Default::default()
        });
        let mut request = RunRequest::new("ROOT");
        request.messages = history;
        request.messages.push(Message::user("ROOT"));
        request.task = task.clone();
        request.metadata = BTreeMap::from([
            (sessions::SESSION_KEY.into(), h.id.clone()),
            (sessions::TURN_KEY.into(), "main".into()),
        ]);
        let root = runtime.start(request).unwrap();
        for _ in 0..200 {
            if service.active_parent(&h.id).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(service.active_parent(&h.id).is_some());
        Self {
            _dir: dir,
            store,
            service,
            host,
            _runtime: runtime,
            model,
            root,
            id: h.id,
            task,
        }
    }
    async fn spawn(
        &self,
        op: &str,
        task: &str,
        role: &str,
        context: ContextMode,
    ) -> Result<subagent::ChildView> {
        self.service
            .spawn(
                &self.root.run_id,
                op,
                Spawn {
                    task: task.into(),
                    role: role.into(),
                    context,
                },
            )
            .await
    }
    async fn wait(&self, id: &str) -> subagent::ChildView {
        let result = self
            .service
            .wait(
                &self.root.run_id,
                &[id.into()],
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(!result.timed_out);
        result.agents.into_iter().next().unwrap()
    }
    async fn finish(mut self) {
        self.model.root.notify_one();
        let report = self.root.wait().await.unwrap();
        assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
        self.service.shutdown().await.unwrap();
        self.host.shutdown().await.unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_children_are_parallel_scoped_and_budgeted() {
    let f = Fixture::new(
        Config {
            max_parallel: 2,
            ..Default::default()
        },
        32,
    )
    .await;
    let a = f
        .spawn("a", "BLOCK", "default", ContextMode::Independent)
        .await
        .unwrap();
    let b = f
        .spawn("b", "BLOCK", "default", ContextMode::Independent)
        .await
        .unwrap();
    assert_eq!(
        f.spawn("c", "CHECK", "default", ContextMode::Independent)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Limit
    );
    let wait = f
        .service
        .wait(
            &f.root.run_id,
            &[a.id.clone(), b.id.clone()],
            Duration::from_millis(20),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(wait.timed_out);
    assert!(f.service.has_active_children(&f.id));
    let other = f.store.create("unrelated").unwrap();
    assert!(f.service.view(&other.id, &a.id).await.is_err());
    let a = f.service.interrupt(&f.id, &a.id, None).await.unwrap();
    assert_eq!(a.status, sessions::Status::Cancelled);
    assert_eq!(
        f.service.view(&f.id, &b.id).await.unwrap().status,
        sessions::Status::Running
    );
    f.service.interrupt(&f.id, &b.id, None).await.unwrap();
    assert!(!f.task.cancellation().is_cancelled());
    assert_eq!(f.task.usage().model_calls, 3);
    f.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_and_fresh_contexts_are_distinct_and_do_not_inherit_delegation() {
    let f = Fixture::new(Config::default(), 32).await;
    let a = f
        .spawn("a", "CHECK", "default", ContextMode::Independent)
        .await
        .unwrap();
    let b = f
        .spawn("b", "CHECK", "default", ContextMode::Fork)
        .await
        .unwrap();
    let a = f.wait(&a.id).await;
    let b = f.wait(&b.id).await;
    assert!(a.output.unwrap().contains("secret=false"));
    assert!(b.output.as_ref().unwrap().contains("secret=true"));
    assert!(b.output.unwrap().contains("delegation=false"));
    assert_eq!(f.store.get(&b.id).unwrap().header.turn_count, 1);
    f.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_do_not_start_idle_work_and_followups_reuse_exact_history() {
    let f = Fixture::new(Config::default(), 32).await;
    let child = f
        .spawn("a", "WRITE", "worker", ContextMode::Independent)
        .await
        .unwrap();
    assert_eq!(f.wait(&child.id).await.status, sessions::Status::Completed);
    let before = f.task.usage().model_calls;
    f.service
        .send_message(&f.root.run_id, "mail", &child.id, "ADDITIONAL_FACT")
        .await
        .unwrap();
    f.service
        .send_message(&f.root.run_id, "mail", &child.id, "ADDITIONAL_FACT")
        .await
        .unwrap();
    assert!(f
        .service
        .send_message(&f.root.run_id, "mail", &child.id, "changed")
        .await
        .is_err());
    assert_eq!(f.task.usage().model_calls, before);
    f.service
        .followup(&f.root.run_id, "next", &child.id, "FOLLOWUP")
        .await
        .unwrap();
    let done = f.wait(&child.id).await;
    assert_eq!(done.turns, 2);
    assert!(done.output.unwrap().contains("mail=true"));
    assert_eq!(f.model.writes.load(Ordering::SeqCst), 1);
    let duplicate = f
        .spawn("a", "WRITE", "worker", ContextMode::Independent)
        .await
        .unwrap();
    assert_eq!(duplicate.id, child.id);
    assert!(f
        .spawn("a", "different", "worker", ContextMode::Independent)
        .await
        .is_err());
    f.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explorer_cannot_dispatch_writes_and_global_call_budget_cannot_reset() {
    let f = Fixture::new(Config::default(), 3).await;
    let a = f
        .spawn("a", "WRITE", "explorer", ContextMode::Independent)
        .await
        .unwrap();
    assert_eq!(f.wait(&a.id).await.status, sessions::Status::Failed);
    assert_eq!(f.model.writes.load(Ordering::SeqCst), 0);
    let b = f
        .spawn("b", "CHECK", "default", ContextMode::Independent)
        .await
        .unwrap();
    f.wait(&b.id).await;
    let c = f
        .spawn("c", "CHECK", "default", ContextMode::Independent)
        .await
        .unwrap();
    assert_eq!(f.wait(&c.id).await.status, sessions::Status::Limited);
    assert_eq!(f.task.usage().model_calls, 3);
    f.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_releases_capacity_and_duplicate_followups_never_replay() {
    let f = Fixture::new(
        Config {
            max_parallel: 1,
            ..Default::default()
        },
        64,
    )
    .await;
    let child = f
        .spawn("first", "CHECK", "default", ContextMode::Independent)
        .await
        .unwrap();
    f.wait(&child.id).await;
    let active = f
        .service
        .followup(&f.root.run_id, "blocked-next", &child.id, "BLOCK")
        .await
        .unwrap();
    let duplicate = f
        .service
        .followup(&f.root.run_id, "blocked-next", &child.id, "BLOCK")
        .await
        .unwrap();
    assert_eq!(duplicate.run_id, active.run_id);
    assert_eq!(duplicate.turns, 2);
    assert!(f
        .service
        .followup(&f.root.run_id, "blocked-next", &child.id, "different")
        .await
        .is_err());
    f.service
        .interrupt(&f.id, &child.id, active.run_id.as_deref())
        .await
        .unwrap();
    for i in 0..8 {
        f.service
            .followup(&f.root.run_id, &format!("next-{i}"), &child.id, "CHECK")
            .await
            .unwrap();
        let settled = f.wait(&child.id).await;
        assert_eq!(settled.status, sessions::Status::Completed);
        assert!(!f.service.has_active_children(&f.id));
    }
    f.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_cancel_stops_owned_children_and_does_not_replay() {
    let mut f = Fixture::new(Config::default(), 32).await;
    let child = f
        .spawn("a", "BLOCK", "default", ContextMode::Independent)
        .await
        .unwrap();
    f.root.cancel();
    let report = tokio::time::timeout(Duration::from_secs(5), f.root.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert!(!f.service.has_active_children(&f.id));
    assert_eq!(
        f.service.view(&f.id, &child.id).await.unwrap().status,
        sessions::Status::Cancelled
    );
    assert!(f
        .service
        .followup(&f.root.run_id, "late", &child.id, "CHECK")
        .await
        .is_err());
    f.service.shutdown().await.unwrap();
    f.host.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_parent_with_unfinished_children_is_not_reported_successfully() {
    let mut f = Fixture::new(Config::default(), 32).await;
    let child = f
        .spawn("a", "BLOCK", "default", ContextMode::Independent)
        .await
        .unwrap();
    f.model.root.notify_one();
    let report = f.root.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(
        f.service.view(&f.id, &child.id).await.unwrap().status,
        sessions::Status::Cancelled
    );
    f.service.shutdown().await.unwrap();
    f.host.shutdown().await.unwrap();
}
