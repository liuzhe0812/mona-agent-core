use api::*;
use futures_util::stream;
use runtime::HostBuilder;
use serde_json::Value;
use skills::{SkillDefinition, SkillProvider, SkillRegistry, SkillSummary, SkillsPlugin};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
struct MemoryProvider {
    id: String,
    entries: BTreeMap<String, (SkillSummary, String)>,
    resources: BTreeMap<(String, String), String>,
    wait_for_cancel: bool,
}

impl MemoryProvider {
    fn new(id: &str, entries: impl IntoIterator<Item = (SkillSummary, String)>) -> Self {
        Self {
            id: id.into(),
            entries: entries
                .into_iter()
                .map(|(summary, content)| (summary.name.clone(), (summary, content)))
                .collect(),
            resources: BTreeMap::new(),
            wait_for_cancel: false,
        }
    }

    fn resource(mut self, name: &str, path: &str, content: &str) -> Self {
        self.resources
            .insert((name.into(), path.into()), content.into());
        self
    }

    fn waits_for_cancel(mut self) -> Self {
        self.wait_for_cancel = true;
        self
    }
}

#[async_trait]
impl SkillProvider for MemoryProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn list(&self, cancel: CancellationToken) -> Result<Vec<SkillSummary>> {
        if self.wait_for_cancel {
            cancel.cancelled().await;
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "provider list cancelled",
            ));
        }
        if cancel.is_cancelled() {
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "provider list cancelled",
            ));
        }
        Ok(self
            .entries
            .values()
            .map(|(summary, _)| summary.clone())
            .collect())
    }

    async fn load(
        &self,
        name: &str,
        _cancel: CancellationToken,
    ) -> Result<Option<SkillDefinition>> {
        Ok(self
            .entries
            .get(name)
            .map(|(summary, content)| SkillDefinition {
                summary: summary.clone(),
                content: content.clone(),
            }))
    }

    async fn read_resource(
        &self,
        name: &str,
        path: &str,
        _cancel: CancellationToken,
    ) -> Result<String> {
        self.resources
            .get(&(name.into(), path.into()))
            .cloned()
            .ok_or_else(|| AgentError::new(ErrorCode::Tool, "resource not found"))
    }
}

struct ScriptedModel {
    replies: Mutex<VecDeque<Vec<ModelEvent>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    fn new(replies: Vec<Vec<ModelEvent>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(vec![]),
        })
    }
}

#[async_trait]
impl Model for ScriptedModel {
    async fn stream(
        &self,
        request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelStream> {
        self.requests.lock().unwrap().push(request);
        let events =
            self.replies.lock().unwrap().pop_front().ok_or_else(|| {
                AgentError::new(ErrorCode::ModelProtocol, "test script exhausted")
            })?;
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

fn summary(name: &str, model_invocable: bool, user_invocable: bool) -> SkillSummary {
    SkillSummary {
        name: name.into(),
        description: format!("{name} description"),
        model_invocable,
        user_invocable,
        location: None,
    }
}

fn answer(text: &str) -> Vec<ModelEvent> {
    vec![
        ModelEvent::Text(text.into()),
        ModelEvent::Finish(FinishReason::Stop),
        ModelEvent::Usage(Usage {
            input_tokens: 1,
            output_tokens: 1,
        }),
        ModelEvent::End,
    ]
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolDelta {
            index: 0,
            id: Some(id.into()),
            name: Some(name.into()),
            arguments: arguments.to_string(),
        },
        ModelEvent::Finish(FinishReason::ToolCalls),
        ModelEvent::Usage(Usage {
            input_tokens: 1,
            output_tokens: 1,
        }),
        ModelEvent::End,
    ]
}

struct ExcludeRead;
#[async_trait]
impl ToolSelector for ExcludeRead {
    async fn select(&self, _: &RunContext, _: usize, _: &[Message], _: &[ToolSpec]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn model_receives_skill_catalog_without_a_dedicated_tool_or_eager_body() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("demo-skill");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::write(
        bundle.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: demo-skill description\n---\nUse this skill body.",
    )
    .unwrap();
    std::fs::write(bundle.join("README.txt"), "Referenced resource text.").unwrap();
    let provider = skills::LocalSkills::new(
        "filesystem",
        vec![directory.path().to_owned()],
        Default::default(),
    )
    .unwrap();
    let registry =
        Arc::new(SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap());
    // The catalog promises ordinary read access. Test absent, usable, and dynamically
    // excluded readers rather than advertising an impossible operation to the model.
    for (registered, excluded) in [(false, false), (true, false), (true, true)] {
        let model = ScriptedModel::new(vec![answer("done")]);
        let mut builder = HostBuilder::new().model(model.clone())
            .plugin(Arc::new(SkillsPlugin::new(registry.clone())));
        if registered {
            let config = tools::ToolConfig::new(directory.path(), "unused-shell");
            builder = builder.tool(Arc::new(tools::ReadTool::new(config)));
        }
        if excluded { builder = builder.tool_selector(Arc::new(ExcludeRead)); }
        let mut host = builder.build().await.unwrap();
        let report = host.engine().execute(RunRequest::new("use the available skill")).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
        assert_eq!(report.output.as_deref(), Some("done"));
        assert_eq!(report.model_requests.len(), 1);
        assert_eq!(report.task_usage.model_calls, 1);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let readable = registered && !excluded;
        let names = requests[0].tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, if readable { vec!["read"] } else { Vec::new() });
        assert_eq!(requests[0].messages.len(), if readable { 2 } else { 1 });
        let catalog = requests[0].messages.iter().map(|m| m.text()).collect::<Vec<_>>().join("\n");
        for marker in ["demo-skill", "demo-skill description", "SKILL.md", "use read"] {
            assert_eq!(catalog.contains(marker), readable, "marker={marker}, reader={readable}");
        }
        assert!(!catalog.contains("Use this skill body."));
        assert!(!catalog.contains("Referenced resource text."));
        drop(requests);
        host.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn model_loads_the_complete_skill_through_the_regular_read_tool() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("demo-skill");
    std::fs::create_dir(&bundle).unwrap();
    let skill_path = bundle.join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: demo-skill\ndescription: demo skill\n---\nFollow the complete body.",
    )
    .unwrap();
    let provider = skills::LocalSkills::new(
        "filesystem",
        vec![directory.path().to_owned()],
        Default::default(),
    )
    .unwrap();
    let registry =
        Arc::new(SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap());
    let model = ScriptedModel::new(vec![
        tool_call(
            "read-skill",
            "read",
            serde_json::json!({"path":skill_path.to_string_lossy()}),
        ),
        answer("done"),
    ]);
    let config = tools::ToolConfig::new(directory.path(), "bash");
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(tools::ReadTool::new(config)))
        .plugin(Arc::new(SkillsPlugin::new(registry)))
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("use demo skill"))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let requests = model.requests.lock().unwrap();
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read"]
    );
    assert!(requests[0]
        .messages
        .iter()
        .any(|message| message.text().contains("<available_skills>")));
    assert!(requests[1].messages.iter().any(|message| matches!(message, Message::Tool { result } if result.content.text().contains("Follow the complete body."))));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn ordinary_runtime_does_not_need_skills_plugin() {
    let model = ScriptedModel::new(vec![answer("plain")]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("plain run"))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.output.as_deref(), Some("plain"));
    assert!(model.requests.lock().unwrap()[0].tools.is_empty());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_allowed_tools_hides_skill_catalog_and_tool() {
    let mut visible = summary("demo-skill", true, true);
    visible.location = Some("/allowed/demo-skill/SKILL.md".into());
    let provider = MemoryProvider::new("memory", [(visible, "body".into())]);
    let registry =
        Arc::new(SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap());
    let model = ScriptedModel::new(vec![answer("no skill")]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .plugin(Arc::new(SkillsPlugin::new(registry)))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("do not use tools");
    request.allowed_tools = Some(Default::default());
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let requests = model.requests.lock().unwrap();
    assert!(requests[0].tools.is_empty());
    assert!(requests[0]
        .messages
        .iter()
        .all(|message| !message.text().contains("available_skills")));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn duplicate_skill_names_are_first_provider_wins_even_when_first_is_disabled() {
    let first = MemoryProvider::new(
        "first",
        [(summary("same-skill", false, true), "first body".into())],
    );
    let second = MemoryProvider::new(
        "second",
        [(summary("same-skill", true, true), "second body".into())],
    );
    let registry =
        SkillRegistry::new(vec![Arc::new(first), Arc::new(second)], Default::default()).unwrap();
    let catalog = registry.list(CancellationToken::new()).await.unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].provider, "first");
    assert!(!catalog[0].summary.model_invocable);
    let error = registry
        .load_for_model("same-skill", CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Policy);
}

#[tokio::test]
async fn model_policy_rejects_disabled_skills_but_ignores_user_invocation_flag() {
    let provider = MemoryProvider::new(
        "memory",
        [
            (summary("user-only", false, true), "user body".into()),
            (summary("model-only", true, false), "model body".into()),
        ],
    )
    .resource("user-only", "README.txt", "user resource")
    .resource("model-only", "README.txt", "model resource");
    let registry = SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap();
    assert_eq!(
        registry
            .load_for_model("user-only", CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Policy
    );
    assert_eq!(
        registry
            .read_resource_for_model("user-only", "README.txt", CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Policy
    );
    assert_eq!(
        registry
            .load_for_model("model-only", CancellationToken::new())
            .await
            .unwrap()
            .content,
        "model body"
    );
    assert_eq!(
        registry
            .read_resource_for_model("model-only", "README.txt", CancellationToken::new())
            .await
            .unwrap(),
        "model resource"
    );
}

#[tokio::test]
async fn cancelling_provider_query_returns_without_waiting_for_provider() {
    let provider = MemoryProvider::new("slow", std::iter::empty()).waits_for_cancel();
    let registry =
        Arc::new(SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap());
    let cancel = CancellationToken::new();
    let query = tokio::spawn({
        let registry = registry.clone();
        let cancel = cancel.clone();
        async move { registry.list(cancel).await }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(200), query)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
}

#[tokio::test]
async fn tools_disabled_run_does_not_publish_skill_catalog() {
    let mut visible = summary("long-skill", true, true);
    visible.location = Some("/allowed/long-skill/SKILL.md".into());
    let provider = MemoryProvider::new("memory", [(visible, "body".into())]);
    let registry =
        Arc::new(SkillRegistry::new(vec![Arc::new(provider)], Default::default()).unwrap());
    let model = ScriptedModel::new(vec![answer("done")]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .plugin(Arc::new(SkillsPlugin::new(registry)))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("load the skill");
    request.enable_tools = false;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.model_requests.len(), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(model.requests.lock().unwrap()[0]
        .messages
        .iter()
        .all(|message| !message.text().contains("available_skills")));
    host.shutdown().await.unwrap();
}
