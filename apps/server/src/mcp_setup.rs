//! Host-owned configuration and secrets; protocol/execution stay in the reusable MCP package.
use api::*;
use mcp::{Config, Server, TransportConfig};
use models::{EncryptedFileStore, SettingsStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
fn invalid(s: &str) -> AgentError {
    AgentError::new(ErrorCode::Configuration, s)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    revision: u64,
    config: Config,
}
pub struct McpHost {
    pub service: Arc<mcp::Service>,
    saved: Mutex<Document>,
    active: Config,
    store: EncryptedFileStore,
    _lease: File,
}
impl McpHost {
    pub async fn from_environment(
        key: Option<String>,
        enabled: bool,
        demo: bool,
    ) -> Result<Arc<Self>> {
        let root = crate::workspace_setup::state_root()
            .map_err(|_| invalid("MCP state directory is unavailable"))?;
        let path = std::env::var_os("AGENT_MCP_SETTINGS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                root.join(if demo {
                    "demo-mcp-settings.enc"
                } else {
                    "mcp-settings.enc"
                })
            });
        let (store, lease, saved) = tokio::task::spawn_blocking(move || {
            let parent = path
                .parent()
                .ok_or_else(|| invalid("MCP settings path requires a parent directory"))?;
            fs::create_dir_all(parent)
                .map_err(|_| invalid("cannot create MCP settings directory"))?;
            for p in [
                &path,
                &path.with_extension("lock"),
                &path.with_extension("key"),
            ] {
                if let Ok(meta) = fs::symlink_metadata(p) {
                    if workspace::is_link(&meta) || !meta.is_file() {
                        return Err(invalid("MCP settings cannot use a link or non-file"));
                    }
                }
            }
            let lease = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path.with_extension("lock"))
                .map_err(|_| invalid("cannot open MCP settings lock"))?;
            lease
                .try_lock()
                .map_err(|_| invalid("MCP settings are owned by another host"))?;
            let key = match std::env::var("AGENT_MCP_STORE_KEY")
                .ok()
                .or(key)
                .or_else(|| std::env::var("AGENT_MODEL_STORE_KEY").ok())
            {
                Some(key) => key,
                None => local_key(&path.with_extension("key"))?,
            };
            let store = EncryptedFileStore::new(path, &key)?;
            let saved = match store.load()? {
                Some(bytes) => serde_json::from_slice::<Document>(&bytes)
                    .map_err(|_| invalid("invalid MCP settings; not reset"))?,
                None => Document {
                    version: 1,
                    revision: 0,
                    config: Config::default(),
                },
            };
            if saved.version != 1 {
                return Err(invalid("unsupported MCP settings version"));
            }
            saved.config.validate()?;
            Ok::<_, AgentError>((store, lease, saved))
        })
        .await
        .map_err(|_| invalid("MCP settings worker failed"))??;
        let active = saved.config.clone();
        let service = mcp::Service::connect(
            if enabled && !demo {
                active.clone()
            } else {
                Config::default()
            },
            CancellationToken::new(),
        )
        .await?;
        Ok(Arc::new(Self {
            service,
            saved: Mutex::new(saved),
            active,
            store,
            _lease: lease,
        }))
    }
    pub fn view(&self, enabled: bool) -> Result<Value> {
        let saved = self
            .saved
            .lock()
            .map_err(|_| invalid("MCP settings unavailable"))?;
        let servers: Vec<_> = saved
            .config
            .servers
            .iter()
            .map(|(id, server)| {
                let mut config = serde_json::to_value(server).expect("serializable MCP config");
                let transport = config
                    .get_mut("transport")
                    .and_then(Value::as_object_mut)
                    .expect("MCP transport object");
                let env_keys = transport
                    .remove("env")
                    .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()))
                    .unwrap_or_default();
                let header_keys = transport
                    .remove("headers")
                    .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()))
                    .unwrap_or_default();
                json!({"id":id,"config":config,"env_keys":env_keys,"header_keys":header_keys})
            })
            .collect();
        Ok(
            json!({"version":1,"revision":saved.revision,"enabled":enabled,"restart_required":saved.config!=self.active,"servers":servers,"status":self.service.views()}),
        )
    }
    pub fn upsert(
        &self,
        revision: u64,
        id: String,
        mut server: Server,
        clear_secrets: bool,
        enabled: bool,
    ) -> Result<Value> {
        let mut state = self
            .saved
            .lock()
            .map_err(|_| invalid("MCP settings unavailable"))?;
        if state.revision != revision {
            return Err(invalid("MCP settings conflict; refresh before saving"));
        }
        if let Some(old) = state.config.servers.get(&id) {
            merge_secrets(old, &mut server, clear_secrets)?;
        }
        let mut next = state.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("MCP revision exhausted"))?;
        next.config.servers.insert(id, server);
        next.config.validate()?;
        self.store.save(
            &serde_json::to_vec(&next).map_err(|_| invalid("MCP settings encoding failed"))?,
        )?;
        *state = next;
        drop(state);
        self.view(enabled)
    }
    pub fn delete(&self, revision: u64, id: &str, enabled: bool) -> Result<Value> {
        let mut state = self
            .saved
            .lock()
            .map_err(|_| invalid("MCP settings unavailable"))?;
        if state.revision != revision {
            return Err(invalid("MCP settings conflict; refresh before deleting"));
        }
        let mut next = state.clone();
        if next.config.servers.remove(id).is_none() {
            return Err(invalid("unknown MCP server"));
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("MCP revision exhausted"))?;
        self.store.save(
            &serde_json::to_vec(&next).map_err(|_| invalid("MCP settings encoding failed"))?,
        )?;
        *state = next;
        drop(state);
        self.view(enabled)
    }
}
fn merge_secrets(old: &Server, new: &mut Server, clear: bool) -> Result<()> {
    match (&old.transport, &mut new.transport) {
        (
            TransportConfig::Stdio {
                command: old_command,
                args: old_args,
                env: old_env,
                cwd: old_cwd,
            },
            TransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            },
        ) => {
            let same = command == old_command && args == old_args && cwd == old_cwd;
            if !clear && !old_env.is_empty() && env.is_empty() {
                if !same {
                    return Err(invalid("MCP executable, args or cwd changed; re-enter environment secrets or explicitly clear them"));
                }
                *env = old_env.clone();
            }
        }
        (
            TransportConfig::StreamableHttp {
                url: old_url,
                headers: old_headers,
                ..
            },
            TransportConfig::StreamableHttp { url, headers, .. },
        ) => {
            if !clear && !old_headers.is_empty() && headers.is_empty() {
                if url != old_url {
                    return Err(invalid(
                        "MCP endpoint changed; re-enter headers or explicitly clear them",
                    ));
                }
                *headers = old_headers.clone();
            }
        }
        _ => {}
    }
    Ok(())
}
fn local_key(path: &Path) -> Result<String> {
    match File::open(path) {
        Ok(file) => {
            let mut key = String::new();
            file.take(129)
                .read_to_string(&mut key)
                .map_err(|_| invalid("cannot read MCP storage key"))?;
            if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(invalid("invalid MCP storage key; not reset"));
            }
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).map_err(|_| invalid("secure random unavailable"))?;
            let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(path)
                .map_err(|_| invalid("cannot create MCP storage key"))?;
            file.write_all(key.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("cannot save MCP storage key"))?;
            Ok(key)
        }
        Err(_) => Err(invalid("cannot read MCP storage key")),
    }
}
/// Local sandbox does not contain remote MCP servers. Do not let read-only mode
/// authorize unknown external side effects; only host-declared read-only tools pass.
pub struct ExternalPolicy;
#[async_trait]
impl ToolPolicy for ExternalPolicy {
    async fn check(
        &self,
        ctx: &RunContext,
        call: &ToolCall,
        spec: &ToolSpec,
    ) -> Result<PolicyDecision> {
        let _ = call;
        if spec.name.starts_with("mcp__")
            && spec.side_effects
            && ctx
                .metadata
                .get("mona.sandbox.mode")
                .is_some_and(|s| s == "read-only")
        {
            return Ok(PolicyDecision::Deny(
                "只读模式不允许有副作用的 MCP 工具；远程工具不在本机沙箱内。".into(),
            ));
        }
        Ok(PolicyDecision::Allow)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_not_reused_for_a_new_endpoint() {
        let old:Server=serde_json::from_value(json!({"transport":{"type":"streamable-http","url":"https://a.example/mcp","headers":{"Authorization":"SECRET"}}})).unwrap();
        let mut new = old.clone();
        if let TransportConfig::StreamableHttp { url, headers, .. } = &mut new.transport {
            *url = "https://b.example/mcp".into();
            headers.clear();
        }
        assert!(merge_secrets(&old, &mut new, false).is_err());
        assert!(merge_secrets(&old, &mut new, true).is_ok());
    }
}
