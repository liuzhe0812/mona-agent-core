use crate::{
    disk, invalid,
    state::{inbox, Profile, ROOT_KEY},
    storage,
    tools::AgentTool,
    Driver, Service, TOOL_NAMES,
};
use api::*;
use serde_json::json;
use std::sync::Arc;
#[derive(Clone)]
pub struct Binding {
    pub(crate) service: Arc<Service>,
    driver: Arc<dyn Driver>,
}
impl Binding {
    pub(crate) fn new(service: Arc<Service>, driver: Arc<dyn Driver>) -> Self {
        Self { service, driver }
    }
    pub fn plugin(&self) -> SubagentPlugin {
        SubagentPlugin(self.clone())
    }
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        TOOL_NAMES
            .iter()
            .map(|name| {
                Arc::new(AgentTool {
                    service: self.service.clone(),
                    name,
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}
pub struct SubagentPlugin(Binding);
#[async_trait]
impl Plugin for SubagentPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("subagent")
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.context_transform(Arc::new(self.0.clone()));
        registrar.tool_selector(Arc::new(self.0.clone()));
        for tool in self.0.tools() {
            registrar.tool(tool)?;
        }
        Ok(())
    }
}
#[async_trait]
impl ToolSelector for Binding {
    async fn select(
        &self,
        ctx: &RunContext,
        _step: usize,
        _history: &[Message],
        available: &[ToolSpec],
    ) -> Result<Vec<String>> {
        Ok(available
            .iter()
            .filter(|t| {
                ctx.metadata.contains_key(sessions::SESSION_KEY)
                    || !TOOL_NAMES.contains(&t.name.as_str())
            })
            .map(|t| t.name.clone())
            .collect())
    }
}
#[async_trait]
impl ContextTransform for Binding {
    async fn sources(&self, ctx: &RunContext, _history: &[Message]) -> Result<Vec<ContextBlock>> {
        let Some(id) = ctx.metadata.get(sessions::SESSION_KEY).cloned() else {
            return Ok(vec![]);
        };
        let store = self.service.store.clone();
        let doc = disk(move || store.get(&id).map_err(storage)).await?;
        let mut blocks = vec![];
        if doc.header.metadata.contains_key(ROOT_KEY) {
            Profile::read(&doc)?;
            let mail = inbox(&doc)?;
            if !mail.is_empty() {
                blocks.push(ContextBlock::new(
                    "subagent.inbox",
                    serde_json::to_string(&mail).map_err(|_| invalid("inbox encoding failed"))?,
                ));
            }
        }
        if ctx.request_tools.iter().any(|t| t.name == "spawn_agent") {
            let roles: Vec<_> = self
                .service
                .config
                .roles
                .iter()
                .map(|(id, r)| json!({"id":id,"description":r.description}))
                .collect();
            blocks.push(ContextBlock::new("subagent.guidance",format!(
                "Delegate only clearly scoped work. spawn_agent starts asynchronously; use wait_agent to collect results before your final reply, or explicitly interrupt unfinished work. send_message only persists supplementary information; followup_agent starts another activation on an idle child. Children share this task's total budget and workspace, not a mutable conversation. Assign nonoverlapping files; do not undo another agent's changes. Child success is not independent verification. A wait timeout does not cancel a child. Parent completion stops unfinished owned children. Max parallel: {}; max depth: {}. Available roles: {}",
                self.service.config.max_parallel,self.service.config.max_depth,serde_json::to_string(&roles).map_err(|_|invalid("roles encoding failed"))?)));
        }
        Ok(blocks)
    }
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        self.service
            .capture(ctx, messages.clone(), self.driver.clone())
            .await?;
        Ok(messages)
    }
    async fn finish(&self, run: &str) -> Result<()> {
        self.service.finish(run).await
    }
}
