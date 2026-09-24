mod support;
use api::*;
use runtime::HostBuilder;
use memory::{InMemoryStore, Backend, Binding, Change, Operation, Origin, MemoryPlugin};
use planner::{PlanRequest, Planner, PlannerPlugin, PLANNER_SERVICE};
use std::{sync::{Arc, Mutex}, time::Duration};
use support::*;

struct TestPlugin {
    id: &'static str, requires: Vec<&'static str>, provides: Vec<&'static str>,
    log: Arc<Mutex<Vec<String>>>, fail: bool, publish: bool, hang: bool,
}
#[async_trait]
impl Plugin for TestPlugin {
    fn manifest(&self) -> PluginManifest {
        let mut manifest = PluginManifest::new(self.id);
        manifest.requires = self.requires.iter().map(|s| (*s).into()).collect();
        manifest.provides = self.provides.iter().map(|s| (*s).into()).collect();
        manifest
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        self.log.lock().unwrap().push(format!("install:{}", self.id));
        for key in &self.requires { assert!(registrar.services().contains(key)); }
        if self.publish {
            for key in &self.provides { registrar.publish(ServiceRegistration::new(*key, Arc::new(42u32)))?; }
        }
        if self.hang { return std::future::pending().await; }
        if self.fail { return Err(AgentError::new(ErrorCode::Plugin, "install failed after staging")); }
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.log.lock().unwrap().push(format!("stop:{}", self.id)); Ok(())
    }
}
fn plugin(id: &'static str, requires: Vec<&'static str>, provides: Vec<&'static str>, log: Arc<Mutex<Vec<String>>>) -> TestPlugin {
    TestPlugin { id, requires, provides, log, fail: false, publish: true, hang: false }
}

#[tokio::test]
async fn services_topologically_order_plugins_and_shutdown_reverses_order() {
    let log = Arc::new(Mutex::new(vec![]));
    let a = Arc::new(plugin("a", vec![], vec!["service.a"], log.clone()));
    let b = Arc::new(plugin("b", vec!["service.a"], vec![], log.clone()));
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![answer("ok")]))
        .plugin(b).plugin(a).build().await.unwrap();
    assert_eq!(*host.services().unwrap().get::<u32>("service.a").unwrap(), 42);
    host.shutdown().await.unwrap();
    assert_eq!(*log.lock().unwrap(), vec!["install:a","install:b","stop:b","stop:a"]);
}
#[tokio::test]
async fn failed_install_rolls_back_partial_plugin_and_predecessors() {
    let log = Arc::new(Mutex::new(vec![]));
    let a = Arc::new(plugin("a", vec![], vec!["a"], log.clone()));
    let mut b = plugin("b", vec!["a"], vec!["b"], log.clone()); b.fail = true;
    let result = HostBuilder::new().model(ScriptModel::new(vec![])).plugin(a).plugin(Arc::new(b)).build().await;
    assert!(result.is_err());
    assert_eq!(*log.lock().unwrap(), vec!["install:a","install:b","stop:b","stop:a"]);
}
#[tokio::test]
async fn missing_dependency_and_cycle_are_rejected_before_any_install() {
    let log = Arc::new(Mutex::new(vec![]));
    let result = HostBuilder::new().model(ScriptModel::new(vec![]))
        .plugin(Arc::new(plugin("a",vec!["b"],vec!["a"],log.clone())))
        .plugin(Arc::new(plugin("b",vec!["a"],vec!["b"],log.clone()))).build().await;
    assert!(result.is_err()); assert!(log.lock().unwrap().is_empty());
    let result = HostBuilder::new().model(ScriptModel::new(vec![]))
        .plugin(Arc::new(plugin("c",vec!["absent"],vec![],log.clone()))).build().await;
    assert!(result.is_err()); assert!(log.lock().unwrap().is_empty());
}
#[tokio::test]
async fn duplicate_service_declarations_are_rejected_before_install() {
    let log = Arc::new(Mutex::new(vec![]));
    let result = HostBuilder::new().model(ScriptModel::new(vec![]))
        .plugin(Arc::new(plugin("a",vec![],vec!["same"],log.clone())))
        .plugin(Arc::new(plugin("b",vec![],vec!["same"],log.clone()))).build().await;
    assert!(result.is_err()); assert!(log.lock().unwrap().is_empty());
}
#[tokio::test]
async fn manifest_must_match_actual_published_services() {
    let log = Arc::new(Mutex::new(vec![]));
    let mut p = plugin("a", vec![], vec!["promised"], log.clone()); p.publish = false;
    assert!(HostBuilder::new().model(ScriptModel::new(vec![])).plugin(Arc::new(p)).build().await.is_err());
    assert_eq!(*log.lock().unwrap(), vec!["install:a", "stop:a"]);
}
#[tokio::test]
async fn installation_timeout_still_calls_shutdown() {
    let log = Arc::new(Mutex::new(vec![]));
    let mut p = plugin("a", vec![], vec![], log.clone()); p.hang = true;
    assert!(HostBuilder::new().model(ScriptModel::new(vec![])).plugin(Arc::new(p))
        .lifecycle_timeout(Duration::from_millis(20)).build().await.is_err());
    assert_eq!(*log.lock().unwrap(), vec!["install:a", "stop:a"]);
}

struct ModelPlugin(Arc<ScriptModel>);
#[async_trait]
impl Plugin for ModelPlugin {
    fn manifest(&self) -> PluginManifest {
        let mut manifest = PluginManifest::new("model-adapter"); manifest.provides.push(MODEL_SERVICE.into()); manifest
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.publish(ServiceRegistration::new(MODEL_SERVICE, Arc::new(ModelProvider(self.0.clone()))))
    }
}
#[tokio::test]
async fn the_model_can_be_supplied_as_a_plugin_service() {
    let mut host = HostBuilder::new().plugin(Arc::new(ModelPlugin(ScriptModel::new(vec![answer("plugin model")]))))
        .build().await.unwrap();
    assert!(!host.services().unwrap().contains(MODEL_SERVICE));
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.output.as_deref(), Some("plugin model"));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn memory_is_a_projection_and_not_a_transcript_rewrite() {
    let store = Arc::new(InMemoryStore::default());
    let cancel = CancellationToken::new();
    store.apply(&Change { revision: store.read(&cancel).unwrap().revision, operations: vec![Operation::Add { text: "Chinese".into() }] }, Origin::host(), &cancel).unwrap();
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![answer("ok")]))
        .plugin(Arc::new(MemoryPlugin::new(vec![Binding::new("personal", store, false)]).unwrap())).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("language preference?")).await.unwrap();
    assert_eq!(report.transcript.len(), 2);
    assert_eq!(report.model_requests[0].request.as_ref().unwrap().messages.len(), 2);
    assert!(report.model_requests[0].request.as_ref().unwrap().messages[0].text().contains("Chinese"));
    assert!(!report.transcript.iter().any(|m| m.text().contains("Curated long-term")));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn memory_write_tool_is_denied_by_default() {
    let store = Arc::new(InMemoryStore::default());
    let cancel = CancellationToken::new();
    let revision = store.read(&cancel).unwrap().revision;
    let model = ScriptModel::new(vec![calls(&[("remember", "memory_update", serde_json::json!({"scope":"personal","revision":revision,"operations":[{"action":"add","text":"fact"}]}))]), answer("denied")]);
    let mut host = HostBuilder::new().model(model).plugin(Arc::new(MemoryPlugin::new(vec![Binding::new("personal", store.clone(), true)]).unwrap())).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("remember")).await.unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Denied);
    assert!(store.read(&cancel).unwrap().entries.is_empty());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn planner_uses_same_executor_and_shared_task_budget() {
    let model = ScriptModel::new(vec![answer(r#"{"steps":["one","two"]}"#), answer("one done"), answer("two done")]);
    let mut host = HostBuilder::new().model(model.clone()).plugin(Arc::new(PlannerPlugin::default())).build().await.unwrap();
    let planner = host.services().unwrap().get::<Planner>(PLANNER_SERVICE).unwrap();
    let mut request = PlanRequest::new("goal");
    request.task = TaskControl::new(TaskLimits { max_model_calls: 2, ..Default::default() });
    let report = planner.plan_and_execute(&host.engine(), request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert!(report.planning_run.model_requests[0].request.as_ref().unwrap().tools.is_empty());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn malformed_plan_is_not_executed() {
    let model = ScriptModel::new(vec![answer(r#"{"steps":[]}"#)]);
    let mut host = HostBuilder::new().model(model.clone()).plugin(Arc::new(PlannerPlugin::default())).build().await.unwrap();
    let planner = host.services().unwrap().get::<Planner>(PLANNER_SERVICE).unwrap();
    let report = planner.plan_and_execute(&host.engine(), PlanRequest::new("goal")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert!(report.step_runs.is_empty());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    host.shutdown().await.unwrap();
}
