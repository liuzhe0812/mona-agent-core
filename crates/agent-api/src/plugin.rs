use crate::{AgentError, ContextTransform, ErrorCode, EventObserver, Model, Result, ResultTransform, Tool, ToolPolicy, ToolSelector, CheckpointSink, API_VERSION};
use async_trait::async_trait;
use std::{any::Any, collections::BTreeMap, sync::Arc};

#[derive(Clone, Default)]
pub struct Services { entries: BTreeMap<String, Arc<dyn Any + Send + Sync>> }
impl Services {
    pub fn without(&self, key: &str) -> Self {
        let mut view = self.clone();
        view.entries.remove(key);
        view
    }
    pub fn contains(&self, key: &str) -> bool { self.entries.contains_key(key) }
    pub fn keys(&self) -> impl Iterator<Item = &String> { self.entries.keys() }
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Result<Arc<T>> {
        let value = self.entries.get(key).ok_or_else(|| AgentError::new(ErrorCode::Dependency, format!("service missing: {key}")))?;
        value.clone().downcast::<T>().map_err(|_| AgentError::new(ErrorCode::Dependency, format!("service type mismatch: {key}")))
    }
    pub fn insert(&mut self, service: ServiceRegistration) -> Result<()> {
        if self.entries.contains_key(&service.key) {
            return Err(AgentError::new(ErrorCode::Dependency, format!("duplicate service: {}", service.key)));
        }
        self.entries.insert(service.key, service.value);
        Ok(())
    }
}

pub struct ServiceRegistration {
    pub key: String,
    pub value: Arc<dyn Any + Send + Sync>,
}
impl ServiceRegistration {
    pub fn new<T: Any + Send + Sync>(key: impl Into<String>, value: Arc<T>) -> Self {
        Self { key: key.into(), value }
    }
}

/// A wrapper makes the trait object usable in the typed service registry.
pub struct ModelProvider(pub Arc<dyn Model>);
pub const MODEL_SERVICE: &str = "agent.model";

#[derive(Clone, Debug)]
pub struct PluginManifest {
    pub id: String,
    pub api_version: u32,
    pub requires: Vec<String>,
    pub provides: Vec<String>,
}
impl PluginManifest {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), api_version: API_VERSION, requires: vec![], provides: vec![] }
    }
}

/// Staging-only registrar. No runtime internals are exposed to a plugin.
pub trait Registrar: Send {
    fn services(&self) -> &Services;
    fn publish(&mut self, registration: ServiceRegistration) -> Result<()>;
    fn tool(&mut self, tool: Arc<dyn Tool>) -> Result<()>;
    fn context_transform(&mut self, transform: Arc<dyn ContextTransform>);
    fn result_transform(&mut self, transform: Arc<dyn ResultTransform>);
    fn policy(&mut self, policy: Arc<dyn ToolPolicy>);
    fn tool_selector(&mut self, selector: Arc<dyn ToolSelector>);
    /// Exactly one sink per Host; duplicate registration is an error, not silent override.
    fn checkpoint_sink(&mut self, sink: Arc<dyn CheckpointSink>) -> Result<()>;
    fn observer(&mut self, observer: Arc<dyn EventObserver>);
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()>;
    /// Called on partial-install rollback, and in reverse order on host shutdown.
    async fn shutdown(&self) -> Result<()> { Ok(()) }
}
