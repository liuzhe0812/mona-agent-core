//! Small, explicit reference catalog; authorization comes from Store, never from this prompt text.
use crate::{disk, Store, SESSION_KEY};
use api::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct SessionReferences {
    store: Arc<Store>,
    cached: std::sync::Mutex<BTreeMap<String, BTreeSet<String>>>,
}
impl SessionReferences {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            cached: Default::default(),
        }
    }
}
fn references(messages: &[Message]) -> BTreeSet<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { result } => result
                .artifact
                .as_ref()
                .filter(|a| a.uri.starts_with("spill:"))
                .map(|a| a.uri.clone()),
            _ => None,
        })
        .collect()
}
#[async_trait]
impl ContextTransform for SessionReferences {
    async fn transform(
        &self,
        _ctx: &RunContext,
        messages: Vec<Message>,
    ) -> api::Result<Vec<Message>> {
        Ok(messages)
    }
    async fn sources(
        &self,
        ctx: &RunContext,
        messages: &[Message],
    ) -> api::Result<Vec<ContextBlock>> {
        if !ctx.tools_enabled
            || ctx
                .allowed_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains("read"))
        {
            return Ok(Vec::new());
        }
        let Some(session) = ctx.metadata.get(SESSION_KEY) else {
            return Ok(Vec::new());
        };
        let saved = self
            .cached
            .lock()
            .map_err(|_| AgentError::new(ErrorCode::Plugin, "session reference state unavailable"))?
            .get(&ctx.run_id)
            .cloned();
        let mut known = if let Some(saved) = saved {
            saved
        } else {
            let store = self.store.clone();
            let session = session.clone();
            let known = disk(move || {
                store
                    .source_history(&session)
                    .map(|history| references(&history))
            })
            .await
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::Checkpoint,
                    "session references could not be loaded",
                )
            })?;
            if known
                .iter()
                .map(|uri| uri.len().saturating_add(1))
                .sum::<usize>()
                > 64 * 1024 - 512
            {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "session artifact catalog exceeds 64 KiB",
                ));
            }
            let mut cached = self.cached.lock().map_err(|_| {
                AgentError::new(ErrorCode::Plugin, "session reference state unavailable")
            })?;
            if ctx.cancel.is_cancelled() {
                return Err(AgentError::new(
                    ErrorCode::Cancelled,
                    "session reference load cancelled",
                ));
            }
            if cached.len() >= 64 {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "too many session reference contexts",
                ));
            }
            cached.insert(ctx.run_id.clone(), known.clone());
            known
        };
        known.extend(references(messages));
        if known.is_empty() {
            return Ok(Vec::new());
        }
        let body = format!("Previously acknowledged artifacts in this conversation. Read with the exact spill: path using read; offset and limit are 1-based UTF-8 bytes. These references do not grant access to other conversations. Expired or removed artifacts may be unavailable; do not invent their contents.\n{}", known.into_iter().collect::<Vec<_>>().join("\n"));
        if body.len() > 64 * 1024 {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "session artifact catalog exceeds 64 KiB",
            ));
        }
        Ok(vec![ContextBlock::new("session.artifacts", body)])
    }
    async fn finish(&self, run: &str) -> api::Result<()> {
        self.cached
            .lock()
            .map_err(|_| AgentError::new(ErrorCode::Plugin, "session reference state unavailable"))?
            .remove(run);
        Ok(())
    }
}
