use crate::{engine::{Engine, EngineConfig}, gate::lifecycle, tools::CompiledTool};
use agent_api::*;
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc, time::Duration};

#[derive(Clone, Default)]
pub(crate) struct Registry {
    pub services: Services,
    pub tools: BTreeMap<String, Arc<CompiledTool>>,
    pub contexts: Vec<Arc<dyn ContextTransform>>,
    pub results: Vec<Arc<dyn ResultTransform>>,
    pub policies: Vec<Arc<dyn ToolPolicy>>,
    pub observers: Vec<Arc<dyn EventObserver>>,
    pub selectors: Vec<Arc<dyn ToolSelector>>,
    pub checkpoint: Option<Arc<dyn CheckpointSink>>,
}
impl Registrar for Registry {
    fn services(&self) -> &Services { &self.services }
    fn publish(&mut self, registration: ServiceRegistration) -> Result<()> { self.services.insert(registration) }
    fn tool(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let compiled = Arc::new(CompiledTool::new(tool)?);
        if self.tools.contains_key(&compiled.spec.name) {
            return Err(AgentError::new(ErrorCode::Dependency, format!("duplicate tool: {}", compiled.spec.name)));
        }
        if self.tools.len() >= 128 { return Err(AgentError::new(ErrorCode::Limit, "maximum 128 tools per host")); }
        self.tools.insert(compiled.spec.name.clone(), compiled);
        Ok(())
    }
    fn context_transform(&mut self, item: Arc<dyn ContextTransform>) { self.contexts.push(item); }
    fn result_transform(&mut self, item: Arc<dyn ResultTransform>) { self.results.push(item); }
    fn policy(&mut self, item: Arc<dyn ToolPolicy>) { self.policies.push(item); }
    fn tool_selector(&mut self, item: Arc<dyn ToolSelector>) { self.selectors.push(item); }
    fn checkpoint_sink(&mut self, sink: Arc<dyn CheckpointSink>) -> Result<()> {
        if self.checkpoint.is_some() { return Err(AgentError::new(ErrorCode::Dependency, "only one checkpoint sink may be installed")); }
        self.checkpoint = Some(sink); Ok(())
    }
    fn observer(&mut self, item: Arc<dyn EventObserver>) { self.observers.push(item); }
}

pub struct HostBuilder {
    registry: Registry,
    model: Option<Arc<dyn Model>>,
    tools: Vec<Arc<dyn Tool>>,
    plugins: Vec<Arc<dyn Plugin>>,
    config: EngineConfig,
    lifecycle_timeout: Duration,
}
impl Default for HostBuilder {
    fn default() -> Self {
        Self { registry: Registry::default(), model: None, tools: vec![], plugins: vec![],
            config: EngineConfig::default(), lifecycle_timeout: Duration::from_secs(5) }
    }
}
impl HostBuilder {
    pub fn new() -> Self { Self::default() }
    pub fn model(mut self, model: Arc<dyn Model>) -> Self { self.model = Some(model); self }
    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self { self.tools.push(tool); self }
    pub fn plugin(mut self, plugin: Arc<dyn Plugin>) -> Self { self.plugins.push(plugin); self }
    pub fn policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self { self.registry.policies.push(policy); self }
    pub fn context_transform(mut self, transform: Arc<dyn ContextTransform>) -> Self { self.registry.contexts.push(transform); self }
    pub fn result_transform(mut self, transform: Arc<dyn ResultTransform>) -> Self { self.registry.results.push(transform); self }
    pub fn tool_selector(mut self, selector: Arc<dyn ToolSelector>) -> Self { self.registry.selectors.push(selector); self }
    pub fn checkpoint_sink(mut self, sink: Arc<dyn CheckpointSink>) -> Result<Self> { self.registry.checkpoint_sink(sink)?; Ok(self) }
    pub fn observer(mut self, observer: Arc<dyn EventObserver>) -> Self { self.registry.observers.push(observer); self }
    /// Explicit host authority. Plugin policies can still veto.
    pub fn allow_side_effect_tool(mut self, name: impl Into<String>) -> Self {
        self.config.allowed_side_effect_tools.insert(name.into()); self
    }
    pub fn max_concurrent_runs(mut self, max: usize) -> Self { self.config.max_concurrent_runs = max; self }
    pub fn event_capacity(mut self, capacity: usize) -> Self { self.config.event_capacity = capacity; self }
    pub fn lifecycle_timeout(mut self, duration: Duration) -> Self { self.lifecycle_timeout = duration; self }

    pub async fn build(mut self) -> Result<Host> {
        if self.config.max_concurrent_runs == 0 || self.config.event_capacity == 0 || self.lifecycle_timeout.is_zero() {
            return Err(AgentError::new(ErrorCode::Configuration, "host limits must be positive"));
        }
        self.registry.services.insert(ServiceRegistration::new("agent.api_version", Arc::new(API_VERSION)))?;
        if let Some(model) = self.model.take() {
            self.registry.services.insert(ServiceRegistration::new(MODEL_SERVICE, Arc::new(ModelProvider(model))))?;
        }
        for tool in self.tools { self.registry.tool(tool)?; }
        let order = sort_plugins(self.plugins, &self.registry.services)?;
        let mut installed: Vec<Arc<dyn Plugin>> = vec![];
        for (manifest, plugin) in order {
            let mut staged = self.registry.clone();
            let before: BTreeSet<_> = staged.services.keys().cloned().collect();
            let result = lifecycle(self.lifecycle_timeout, plugin.install(&mut staged)).await;
            let result = result.and_then(|_| {
                let published: BTreeSet<_> = staged.services.keys().filter(|key| !before.contains(*key)).cloned().collect();
                let declared: BTreeSet<_> = manifest.provides.iter().cloned().collect();
                if published != declared {
                    Err(AgentError::new(ErrorCode::Dependency, format!("{} published services differ from its manifest", manifest.id)))
                } else { Ok(()) }
            });
            if let Err(mut error) = result {
                // Stop the partially initialized plugin too, then unwind earlier plugins.
                installed.push(plugin);
                let cleanup = stop_plugins(&mut installed, self.lifecycle_timeout).await;
                if !cleanup.is_empty() { error.message.push_str(&format!("; rollback: {}", cleanup.join("; "))); }
                return Err(error);
            }
            self.registry = staged;
            installed.push(plugin);
        }
        if let Err(error) = self.registry.services.get::<ModelProvider>(MODEL_SERVICE) {
            let _ = stop_plugins(&mut installed, self.lifecycle_timeout).await;
            return Err(error);
        }
        let engine = Engine::new(self.registry, self.config);
        Ok(Host { engine, installed, lifecycle_timeout: self.lifecycle_timeout })
    }
}

pub struct Host {
    engine: Engine,
    installed: Vec<Arc<dyn Plugin>>,
    lifecycle_timeout: Duration,
}
impl Host {
    pub fn engine(&self) -> Engine { self.engine.clone() }
    pub fn services(&self) -> Result<Services> { self.engine.services() }
    /// Stop admission, cancel and drain runs, revoke registry, then stop plugins in reverse order.
    /// If draining times out, resources stay installed. Call shutdown again after work settles.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.engine.close(self.lifecycle_timeout).await?;
        let errors = stop_plugins(&mut self.installed, self.lifecycle_timeout).await;
        if errors.is_empty() { Ok(()) } else { Err(AgentError::new(ErrorCode::Plugin, errors.join("; "))) }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        // Drop cannot await plugin cleanup. Applications must explicitly call shutdown().
        self.engine.cancel_all();
    }
}

async fn stop_plugins(installed: &mut Vec<Arc<dyn Plugin>>, duration: Duration) -> Vec<String> {
    let mut errors = vec![];
    while let Some(plugin) = installed.pop() {
        if let Err(error) = lifecycle(duration, plugin.shutdown()).await { errors.push(error.to_string()); }
    }
    errors
}

fn sort_plugins(plugins: Vec<Arc<dyn Plugin>>, initial: &Services) -> Result<Vec<(PluginManifest, Arc<dyn Plugin>)>> {
    let mut ids = BTreeSet::new();
    let mut provided: BTreeSet<_> = initial.keys().cloned().collect();
    let mut pending = vec![];
    for plugin in plugins {
        let manifest = plugin.manifest();
        if manifest.api_version != API_VERSION || manifest.id.is_empty() || !ids.insert(manifest.id.clone()) {
            return Err(AgentError::new(ErrorCode::Dependency, "plugin API version/id invalid or duplicate"));
        }
        for key in &manifest.provides {
            if key.is_empty() || !provided.insert(key.clone()) {
                return Err(AgentError::new(ErrorCode::Dependency, format!("duplicate/empty service declaration: {key}")));
            }
        }
        pending.push((manifest, plugin));
    }
    for (manifest, _) in &pending {
        for key in &manifest.requires {
            if !provided.contains(key) {
                return Err(AgentError::new(ErrorCode::Dependency, format!("{} requires absent service {key}", manifest.id)));
            }
        }
    }
    let mut available: BTreeSet<_> = initial.keys().cloned().collect();
    let mut ordered = vec![];
    while !pending.is_empty() {
        let position = pending.iter().position(|(manifest, _)| manifest.requires.iter().all(|key| available.contains(key)))
            .ok_or_else(|| AgentError::new(ErrorCode::Dependency, "plugin dependency cycle"))?;
        let entry = pending.remove(position);
        available.extend(entry.0.provides.iter().cloned());
        ordered.push(entry);
    }
    Ok(ordered)
}
