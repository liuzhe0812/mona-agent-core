//! Product policy binding. The native implementation lives in the independent sandbox package.
use sessions::{Document, SessionError, SessionErrorCode, SessionResult};
pub const STATE_KEY: &str = "sandbox";
pub const MODE_KEY: &str = "mona.sandbox.mode";
fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::new(SessionErrorCode::InvalidRequest, message)
}

/// Kept in no-sandbox builds too: removing an extension never authorizes a saved confined task.
fn stored_mode(doc: &Document) -> SessionResult<Option<&str>> {
    let value = if let Some(value) = doc.body.state.get(STATE_KEY) {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("已保存的沙箱状态无效。"))?;
        if object.len() != 2 || object.get("version").and_then(|v| v.as_u64()) != Some(1) {
            return Err(invalid("已保存的沙箱状态版本或字段无效。"));
        }
        Some(
            object
                .get("mode")
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid("已保存的沙箱模式无效。"))?,
        )
    } else {
        doc.body
            .checkpoint
            .as_ref()
            .and_then(|cp| cp.metadata.get(MODE_KEY))
            .map(String::as_str)
    };
    if value.is_some_and(|m| !matches!(m, "read-only" | "workspace-write" | "danger-full-access")) {
        return Err(invalid("已保存的沙箱模式不受支持。"));
    }
    Ok(value)
}
pub fn require_unconfined(doc: &Document) -> SessionResult<()> {
    if stored_mode(doc)?.is_some_and(|m| m != "danger-full-access") {
        return Err(invalid(
            "此会话要求沙箱，但当前宿主未启用；请重新启用沙箱后继续，不会自动取消限制。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod guard_tests {
    use super::*;
    #[test]
    fn disabled_or_uncompiled_host_rejects_saved_confinement_without_a_runtime() {
        let root = tempfile::tempdir().unwrap();
        let store = sessions::Store::open(&root.path().join("state"), root.path()).unwrap();
        let h = store.create("guard").unwrap();
        assert!(require_unconfined(&store.get(&h.id).unwrap()).is_ok());
        for mode in [
            "read-only",
            "workspace-write",
            "danger-full-access",
            "invalid",
        ] {
            let h = store.header(&h.id).unwrap();
            store
                .update_state(&h.id, h.revision, STATE_KEY, |_| {
                    Ok(serde_json::json!({"version":1,"mode":mode}))
                })
                .unwrap();
            assert_eq!(
                require_unconfined(&store.get(&h.id).unwrap()).is_ok(),
                mode == "danger-full-access"
            );
        }
    }
}

#[cfg(feature = "sandbox")]
mod enabled {
    use super::*;
    use api::{
        async_trait, AgentError, CancellationToken, CheckpointSink, ContextBlock, ContextTransform,
        ErrorCode, Message, RunCheckpoint, RunContext,
    };
    use sandbox::{BackendInfo, LocalSandbox, Mode, Policy, Runner};
    use serde::{Deserialize, Serialize};
    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    #[cfg(test)]
    mod tests {
        include!("sandbox_tests.rs");
    }

    pub struct SandboxHost {
        pub provider: Arc<LocalSandbox>,
        default_mode: Mode,
        locked: bool,
    }
    impl SandboxHost {
        pub fn from_environment() -> sandbox::Result<Self> {
            let configured = match std::env::var("AGENT_SANDBOX_MODE") {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(_) => {
                    return Err(sandbox::Error::new(
                        sandbox::ErrorCode::SandboxConfiguration,
                        "AGENT_SANDBOX_MODE is not valid text",
                    ))
                }
            };
            let default_mode = configured
                .as_deref()
                .map(str::parse)
                .transpose()?
                .unwrap_or(Mode::WorkspaceWrite);
            let executable = std::env::current_exe().map_err(|e| {
                sandbox::Error::new(sandbox::ErrorCode::SandboxConfiguration, e.to_string())
            })?;
            Ok(Self {
                provider: Arc::new(LocalSandbox::new(Runner::embedded(executable))),
                default_mode,
                locked: configured.is_some(),
            })
        }
        fn mode(&self, doc: Option<&Document>, active: bool) -> SessionResult<Mode> {
            let saved = doc.map(stored_mode).transpose()?.flatten();
            if self.locked && active {
                return Ok(self.default_mode);
            }
            saved
                .map(str::parse)
                .transpose()
                .map_err(|e: sandbox::Error| invalid(e.to_string()))
                .map(|mode| {
                    mode.unwrap_or(if active {
                        self.default_mode
                    } else {
                        Mode::DangerFullAccess
                    })
                })
        }
        pub fn metadata(
            &self,
            doc: &Document,
            active: bool,
        ) -> SessionResult<BTreeMap<String, String>> {
            if !active {
                require_unconfined(doc)?;
                return Ok(BTreeMap::new());
            }
            Ok(BTreeMap::from([(
                MODE_KEY.into(),
                self.mode(Some(doc), true)?.as_str().into(),
            )]))
        }
        pub fn binding(&self, root: PathBuf) -> tools::SandboxBinding {
            let default = self.default_mode;
            tools::SandboxBinding::new(self.provider.clone(), move |run| {
                let mode = run
                    .metadata
                    .get(MODE_KEY)
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(agent_error)?
                    .unwrap_or(default);
                let policy = Policy::new(mode, &root).map_err(agent_error)?;
                // A temporary side Run shares the parent's policy, not its writable Session identity.
                if let Some(id) = run.metadata.get(sessions::SESSION_KEY) {
                    policy.with_session(id).map_err(agent_error)
                } else {
                    Ok(policy)
                }
            })
        }
        pub async fn view(
            &self,
            doc: Option<Document>,
            enabled: bool,
        ) -> SessionResult<SandboxView> {
            let mode = self.mode(doc.as_ref(), enabled)?;
            let (backend, unavailable) = if enabled && mode.confined() {
                match self.provider.backend(&CancellationToken::new()).await {
                    Ok(info) => (Some(info), None),
                    Err(e) => (None, Some(e.to_string())),
                }
            } else {
                (None, None)
            };
            Ok(SandboxView {
                version: 1,
                session_id: doc.as_ref().map(|d| d.header.id.clone()),
                revision: doc.as_ref().map(|d| d.header.revision),
                status: doc
                    .as_ref()
                    .map_or(sessions::Status::Idle, |d| d.header.status),
                enabled,
                locked: self.locked,
                mode,
                backend,
                unavailable,
            })
        }
        pub fn change(
            &self,
            store: &sessions::Store,
            id: &str,
            request: SandboxAction,
            enabled: bool,
        ) -> SessionResult<()> {
            if !enabled {
                return Err(invalid("沙箱当前未启用，不能变更模式。"));
            }
            if self.locked {
                return Err(invalid("沙箱模式由部署配置锁定。"));
            }
            store.update_state(id, request.revision, STATE_KEY, |doc| {
                stored_mode(doc)?;
                Ok(serde_json::json!({"version":1,"mode":request.mode}))
            })?;
            Ok(())
        }
    }
    fn agent_error(e: sandbox::Error) -> AgentError {
        AgentError::new(ErrorCode::Configuration, e.to_string())
    }
    #[derive(Serialize)]
    pub struct SandboxView {
        version: u32,
        session_id: Option<String>,
        revision: Option<u64>,
        status: sessions::Status,
        enabled: bool,
        locked: bool,
        mode: Mode,
        backend: Option<BackendInfo>,
        unavailable: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SandboxAction {
        revision: u64,
        mode: Mode,
    }

    pub struct SandboxContext(pub tools::SandboxBinding);
    #[async_trait]
    impl ContextTransform for SandboxContext {
        async fn sources(&self, ctx: &RunContext, _: &[Message]) -> api::Result<Vec<ContextBlock>> {
            let policy = self.0.resolve(ctx)?;
            Ok(vec![ContextBlock::new("sandbox.policy", format!(
                "Current sandbox mode: {}. Workspace: {}. This policy governs Shell and direct write/edit file effects, not network or file reads. Workspace-write permits the workspace and backend temporary area; read-only refuses file mutations. Permission escalation is unavailable: do not retry denied operations with wider permissions. A failed command may already have performed earlier allowed actions.",
                policy.mode.as_str(), policy.workspace_root.display()))])
        }
        async fn transform(
            &self,
            _: &RunContext,
            messages: Vec<Message>,
        ) -> api::Result<Vec<Message>> {
            Ok(messages)
        }
    }

    /// Plan projection, sandbox policy and exact checkpoint share ONE synchronous file transaction.
    pub struct ProductSink(pub Arc<sessions::Store>);
    #[async_trait]
    impl CheckpointSink for ProductSink {
        async fn commit(
            &self,
            cp: Arc<RunCheckpoint>,
            cancel: CancellationToken,
        ) -> api::Result<()> {
            let store = self.0.clone();
            tokio::task::spawn_blocking(move || {
                #[cfg(feature = "planner")]
                let mut patch = crate::planning::checkpoint_state(&cp)?;
                #[cfg(not(feature = "planner"))]
                let mut patch = sessions::HostState::new();
                if let Some(mode) = cp.metadata.get(MODE_KEY) {
                    let mode = mode.parse::<Mode>().map_err(agent_error)?;
                    patch.insert(
                        STATE_KEY.into(),
                        serde_json::json!({"version":1,"mode":mode}),
                    );
                }
                store
                    .commit_with_state(&cp, &cancel, &patch)
                    .map_err(|e| AgentError::new(ErrorCode::Checkpoint, e.message))
            })
            .await
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::Checkpoint,
                    "sandbox/session persistence worker failed",
                )
            })?
        }
    }
}
#[cfg(feature = "sandbox")]
pub use enabled::*;
