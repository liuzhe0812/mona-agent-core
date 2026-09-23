//! Host-side directory binding. Reuses Runtime without global cwd changes or a second loop.
use crate::{capabilities, tool_setup};
use api::*;
use runtime::{Host, HostBuilder};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, Mutex as AsyncMutex};
type HostResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn config_error(e: impl std::fmt::Display) -> AgentError {
    AgentError::new(ErrorCode::Configuration, e.to_string())
}

pub struct Factory {
    pub store: Arc<sessions::Store>,
    pub capabilities: capabilities::CapabilityManager,
    pub demo: bool,
    #[cfg(feature = "memory")]
    pub memory: Option<Arc<crate::memory_routes::MemoryHost>>,
    #[cfg(feature = "history-search")]
    pub history: Option<Arc<sessions::search::HistorySearch>>,
    fixed: Option<Arc<dyn Model>>,
    shell: tools::ShellConfig,
    #[cfg(feature = "model-management")]
    pub manager: Option<models::ModelManager>,
    #[cfg(feature = "spill")]
    pub spill: Option<crate::spill_setup::SpillHost>,
    #[cfg(feature = "compaction")]
    compactor: Option<compaction::Compactor>,
}
impl Factory {
    pub async fn from_environment(
        store: Arc<sessions::Store>,
        capabilities: capabilities::CapabilityManager,
        demo: bool,
    ) -> HostResult<Self> {
        #[cfg(feature = "model-management")]
        let manager = if !demo && capabilities.active("model-management") {
            Some(crate::model_settings::from_environment().map_err(config_error)?)
        } else {
            None
        };
        #[cfg(feature = "model-management")]
        let fixed_model = manager.is_none();
        #[cfg(not(feature = "model-management"))]
        let fixed_model = true;
        let fixed: Option<Arc<dyn Model>> = if !fixed_model {
            None
        } else if demo {
            Some(Arc::new(crate::DemoModel))
        } else {
            let protocol = providers::Protocol::parse(
                &std::env::var("AGENT_MODEL_PROTOCOL")
                    .unwrap_or_else(|_| "chat_completions".into()),
            )?;
            let mut config = providers::ProviderConfig::new(
                protocol,
                std::env::var("AGENT_MODEL_ENDPOINT")?,
                std::env::var("AGENT_MODEL_NAME")?,
            );
            config.api_key = std::env::var("AGENT_MODEL_KEY")
                .ok()
                .filter(|s| !s.is_empty());
            config.context_window_tokens = std::env::var("AGENT_MODEL_CONTEXT_TOKENS")
                .ok()
                .map(|s| s.parse::<u64>())
                .transpose()?;
            config.allow_http_loopback =
                std::env::var("AGENT_ALLOW_HTTP_LOOPBACK").as_deref() == Ok("1");
            if let Ok(extra) = std::env::var("AGENT_MODEL_EXTRA_JSON") {
                config.extra_body = serde_json::from_str(&extra)?;
            }
            Some(providers::create_model(config)?)
        };
        #[cfg(feature = "spill")]
        let spill = if !demo && capabilities.active(capabilities::SPILL) {
            let s = crate::spill_setup::SpillHost::from_environment().map_err(config_error)?;
            s.cleanup_startup().await?;
            Some(s)
        } else {
            None
        };
        #[cfg(feature = "compaction")]
        let compactor = if !demo && capabilities.active(capabilities::COMPACTION) {
            let c = compaction::Compactor::default();
            store.attach_compactor(c.clone())?;
            Some(c)
        } else {
            None
        };
        #[cfg(feature = "memory")]
        let memory = if !demo && capabilities.active(capabilities::MEMORY) {
            Some(tokio::task::spawn_blocking(crate::memory_routes::MemoryHost::from_environment).await??)
        } else { None };
        #[cfg(feature = "history-search")]
        let history = if !demo && capabilities.active(capabilities::HISTORY_SEARCH) {
            let source = store.clone();
            Some(tokio::task::spawn_blocking(move || sessions::search::HistorySearch::open(source)).await??)
        } else { None };
        Ok(Self {
            store,
            #[cfg(feature = "memory")]
            memory,
            #[cfg(feature = "history-search")]
            history,
            capabilities,
            demo,
            fixed,
            shell: tool_setup::shell_from_environment().map_err(config_error)?,
            #[cfg(feature = "model-management")]
            manager,
            #[cfg(feature = "spill")]
            spill,
            #[cfg(feature = "compaction")]
            compactor,
        })
    }
    async fn build(&self, cwd: Option<&Path>) -> Result<Arc<Environment>> {
        let root = cwd
            .map(workspace::canonical_root)
            .transpose()
            .map_err(config_error)?;
        // No-directory environment is solely a model validator; it cannot execute tasks.
        let mut config =
            tools::ToolConfig::new(root.clone().unwrap_or_default(), self.shell.clone());
        #[cfg(feature = "spill")]
        let spill_plugin = self.spill.as_ref().map(|s| {
            let plugin = s.plugin();
            config.read_extensions.push(Arc::new(
                s.read_extension().with_sessions(self.store.clone()),
            ));
            config.output_archive = Some(Arc::new(crate::spill_setup::SpillOutputArchive::new(
                plugin.archive(),
            )));
            plugin
        });
        let mut builder = HostBuilder::new()
            .checkpoint_sink(Arc::new(sessions::SessionSink(self.store.clone())))?;
        if root.is_some() {
            for tool in tools::core_tools(&config) {
                builder = builder.tool(tool);
            }
        }
        for name in [capabilities::GREP, capabilities::FIND, capabilities::LS] {
            if root.is_some() && self.capabilities.active(name) {
                builder = builder.tool(tools::optional_tool(name, &config).expect("known tool"));
            }
        }
        if root.is_some() {
            for name in ["shell", "edit", "write"] {
                builder = builder.allow_side_effect_tool(name);
            }
        }
        #[cfg(feature = "memory")]
        if let (Some(root), Some(memory)) = (&root, &self.memory) {
            let writable = self.capabilities.active(capabilities::MEMORY_UPDATE);
            let memory = memory.clone(); let cwd = root.clone();
            let plugin = tokio::task::spawn_blocking(move || memory.plugin(&cwd, writable)).await.map_err(config_error)??;
            builder = builder.plugin(Arc::new(plugin));
            if writable { builder = builder.allow_side_effect_tool("memory_update"); }
        }
        #[cfg(feature = "history-search")]
        if let (Some(root), Some(search)) = (&root, &self.history) {
            let cwd = root.to_string_lossy().into_owned(); let store = self.store.clone();
            let scope: sessions::search::ScopeResolver = Arc::new(move |ctx| {
                if let Some(id) = ctx.metadata.get(sessions::SESSION_KEY) {
                    let header = store.header(id).map_err(config_error)?;
                    if header.workspace != cwd { return Err(config_error("历史检索目录未绑定。")); }
                    Ok(sessions::search::Scope::Metadata { key: "project.id".into(), value: header.metadata.get("project.id").cloned() })
                } else { Ok(sessions::search::Scope::Workspace(cwd.clone())) }
            });
            for tool in sessions::search::tools(search.clone(), scope) { builder = builder.tool(tool); }
        }
        if root.is_some() && !self.demo && self.capabilities.active(capabilities::INSTRUCTIONS) {
            let store = self.store.clone();
            let rules =
                instructions::ProjectInstructions::new(root.as_ref().expect("root checked"))?
                    .with_history(Arc::new(move |ctx: &RunContext| {
                        match ctx.metadata.get(sessions::SESSION_KEY) {
                            Some(id) => store.source_history(id).map_err(config_error),
                            None => Ok(Vec::new()),
                        }
                    }));
            builder = builder.plugin(Arc::new(rules.plugin()));
        }
        #[cfg(feature = "compaction")]
        if let Some(c) = &self.compactor {
            builder = builder.context_transform(Arc::new(c.clone()));
        }
        #[cfg(feature = "spill")]
        if let Some(plugin) = spill_plugin {
            builder = builder.plugin(Arc::new(plugin)).context_transform(Arc::new(
                sessions::references::SessionReferences::new(self.store.clone()),
            ));
        }
        #[cfg(feature = "skills")]
        if root.is_some() && !self.demo && self.capabilities.active("skills") {
            if let Some(plugin) =
                crate::skill_setup::from_environment(root.as_ref().expect("root checked"))
                    .map_err(config_error)?
            {
                builder = builder.plugin(Arc::new(plugin));
            }
        }
        #[cfg(feature = "model-management")]
        if let Some(manager) = &self.manager {
            builder = builder.plugin(manager.plugin());
        }
        if let Some(model) = &self.fixed {
            builder = builder.model(model.clone());
        }
        let host = builder.build().await?;
        let runtime: Arc<dyn AgentRuntime> = Arc::new(host.engine());
        #[cfg(feature = "model-management")]
        let runtime = if let Some(manager) = &self.manager {
            manager.runtime(runtime)
        } else {
            runtime
        };
        let runtime = sessions::runtime(runtime, self.store.clone());
        Ok(Arc::new(Environment {
            runtime,
            host: AsyncMutex::new(host),
        }))
    }
}
struct Environment {
    runtime: Arc<dyn AgentRuntime>,
    host: AsyncMutex<Host>,
}
struct HeldRun {
    inner: Arc<dyn RunSession>,
    _environment: Arc<Environment>,
}
#[async_trait]
impl RunSession for HeldRun {
    fn run_id(&self) -> &str {
        self.inner.run_id()
    }
    fn cancel(&self) {
        self.inner.cancel();
    }
    fn snapshot(&self) -> RunSnapshot {
        self.inner.snapshot()
    }
    fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.inner.subscribe()
    }
    async fn steer(&self, text: String) -> Result<()> {
        self.inner.steer(text).await
    }
    async fn wait(&self) -> Result<Arc<RunReport>> {
        self.inner.wait().await
    }
}
pub struct DirectoryRuntime {
    default: Arc<Environment>,
    default_root: PathBuf,
    default_ready: bool,
    bindings: Mutex<HashMap<String, Arc<Environment>>>,
}
#[async_trait]
impl AgentExecutor for DirectoryRuntime {
    async fn execute(&self, request: RunRequest) -> Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}
impl AgentRuntime for DirectoryRuntime {
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        self.default.runtime.validate_history(messages, options)
    }
    fn start(&self, request: RunRequest) -> Result<RunHandle> {
        let environment = match request.metadata.get(sessions::SESSION_KEY) {
            Some(id) => self
                .bindings
                .lock()
                .map_err(config_error)?
                .get(id)
                .cloned()
                .ok_or_else(|| config_error("会话执行目录未绑定，任务未启动。"))?,
            None => {
                if !self.default_ready || workspace::canonical_root(&self.default_root).is_err() {
                    return Err(config_error(
                        "默认工作目录不可用；请在设置中确认路径并使用持久会话。",
                    ));
                }
                self.default.clone()
            }
        };
        let (inner, events) = environment.runtime.start(request)?.into_parts();
        Ok(RunHandle::new(
            Arc::new(HeldRun {
                inner,
                _environment: environment,
            }),
            events,
        ))
    }
}
struct Binding {
    runtime: Arc<DirectoryRuntime>,
    id: String,
}
impl Drop for Binding {
    fn drop(&mut self) {
        if let Ok(mut bindings) = self.runtime.bindings.lock() {
            bindings.remove(&self.id);
        }
    }
}
pub struct Environments {
    pub factory: Factory,
    pub runtime: Arc<DirectoryRuntime>,
    pool: AsyncMutex<HashMap<PathBuf, Arc<Environment>>>,
    admission: AsyncMutex<bool>,
}
impl Environments {
    pub async fn new(factory: Factory, default: &Path) -> Result<Arc<Self>> {
        let ready = workspace::canonical_root(default).is_ok();
        let environment = factory.build(ready.then_some(default)).await?;
        Ok(Arc::new(Self {
            factory,
            runtime: Arc::new(DirectoryRuntime {
                default: environment,
                default_root: default.into(),
                default_ready: ready,
                bindings: Mutex::new(HashMap::new()),
            }),
            pool: AsyncMutex::new(HashMap::new()),
            admission: AsyncMutex::new(false),
        }))
    }
    async fn environment(&self, cwd: &Path) -> Result<Arc<Environment>> {
        let cwd = workspace::canonical_root(cwd).map_err(config_error)?;
        let mut pool = self.pool.lock().await;
        if let Some(env) = pool.get(&cwd) {
            return Ok(env.clone());
        }
        if pool.len() >= 64 {
            let idle = pool
                .iter()
                .find(|(_, e)| Arc::strong_count(e) == 1)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| config_error("执行环境数量达到上限，请等待已有任务结束。"))?;
            if let Some(e) = pool.remove(&idle) {
                e.host.lock().await.shutdown().await?;
            }
        }
        let env = self.factory.build(Some(&cwd)).await?;
        pool.insert(cwd, env.clone());
        Ok(env)
    }
    /// Own the complete admission even when the caller's HTTP connection disappears.
    pub async fn start(
        self: &Arc<Self>,
        settings: Arc<crate::workspace_setup::WorkspaceSettings>,
        tasks: application::sessions::SessionApplication,
        id: String,
        request: application::sessions::TurnRequest,
    ) -> application::ApplicationResult<application::sessions::TurnResponse> {
        let this = self.clone();
        tokio::spawn(async move {
            let gate = this.admission.lock().await;
            if *gate {
                return Err(application::ApplicationError::new(
                    application::ApplicationErrorCode::Closed,
                    "宿主正在关闭，未启动任务。",
                ));
            }
            let store = this.factory.store.clone();
            let sid = id.clone();
            let header = tokio::task::spawn_blocking(move || store.header(&sid))
                .await
                .map_err(|_| {
                    application::ApplicationError::new(
                        application::ApplicationErrorCode::Internal,
                        "会话读取失败。",
                    )
                })??;
            let cwd = settings
                .bound_root(Path::new(&header.workspace))
                .map_err(|e| {
                    application::ApplicationError::new(
                        application::ApplicationErrorCode::InvalidRequest,
                        e.message,
                    )
                })?;
            let environment = this
                .environment(&cwd)
                .await
                .map_err(application::ApplicationError::from)?;
            this.runtime
                .bindings
                .lock()
                .map_err(|_| {
                    application::ApplicationError::new(
                        application::ApplicationErrorCode::Internal,
                        "目录绑定失败。",
                    )
                })?
                .insert(id.clone(), environment);
            let _binding = Binding {
                runtime: this.runtime.clone(),
                id: id.clone(),
            };
            tasks.start_turn(id, request).await
        })
        .await
        .map_err(|_| {
            application::ApplicationError::new(
                application::ApplicationErrorCode::Internal,
                "启动状态未知，请刷新会话确认。",
            )
        })?
    }
    pub async fn shutdown(&self) -> Result<()> {
        let mut gate = self.admission.lock().await;
        *gate = true;
        let mut pool = self.pool.lock().await;
        let keys: Vec<_> = pool.keys().cloned().collect();
        for key in keys {
            pool[&key].host.lock().await.shutdown().await?;
            pool.remove(&key);
        }
        self.runtime.default.host.lock().await.shutdown().await
    }
}
