//! Product configuration and a thin driver over the already assembled Runtime.
use api::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use subagent::{Config, Driver, Launch, ModelRoute, Role};
fn error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Configuration, message)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub revision: u64,
    pub config: Config,
}
#[derive(Serialize)]
pub struct SettingsView {
    pub version: u32,
    pub revision: u64,
    pub enabled: bool,
    pub locked: bool,
    pub restart_required: bool,
    pub active: Config,
    pub config: Config,
}
pub struct SubagentHost {
    pub service: Arc<subagent::Service>,
    path: PathBuf,
    state: Mutex<Settings>,
    locked: bool,
    _lease: File,
}
impl SubagentHost {
    pub fn from_environment(store: Arc<sessions::Store>, demo: bool) -> Result<Arc<Self>> {
        let root = crate::workspace_setup::state_root().map_err(|e| error(e.message))?;
        let path = std::env::var_os("AGENT_SUBAGENT_SETTINGS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                root.join(if demo {
                    "demo-subagent-settings.json"
                } else {
                    "subagent-settings.json"
                })
            });
        let override_config = std::env::var("AGENT_SUBAGENT_CONFIG")
            .ok()
            .map(|s| {
                if s.len() > 65536 {
                    return Err(error("subagent deployment config is too large"));
                }
                serde_json::from_str::<Config>(&s)
                    .map_err(|_| error("invalid subagent deployment config"))
            })
            .transpose()?;
        Self::open(store, path, override_config)
    }
    pub fn open(
        store: Arc<sessions::Store>,
        path: PathBuf,
        override_config: Option<Config>,
    ) -> Result<Arc<Self>> {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| error("subagent settings path must have a directory"))?,
        )
        .map_err(|_| error("cannot create subagent settings directory"))?;
        for p in [&path, &path.with_extension("lock")] {
            if let Ok(meta) = fs::symlink_metadata(p) {
                if workspace::is_link(&meta) || !meta.is_file() {
                    return Err(error("subagent settings cannot be a link"));
                }
            }
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(|_| error("subagent settings lock unavailable"))?;
        lease
            .try_lock()
            .map_err(|_| error("subagent settings already owned by another host"))?;
        let mut state = match File::open(&path) {
            Ok(file) => {
                let mut bytes = vec![];
                file.take(65537)
                    .read_to_end(&mut bytes)
                    .map_err(|_| error("cannot read subagent settings"))?;
                if bytes.len() > 65536 {
                    return Err(error("subagent settings exceed 64 KiB"));
                }
                serde_json::from_slice::<Settings>(&bytes)
                    .map_err(|_| error("invalid subagent settings; not reset"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings {
                version: 1,
                revision: 0,
                config: Config::default(),
            },
            Err(_) => return Err(error("cannot read subagent settings")),
        };
        if state.version != 1 {
            return Err(error("unsupported subagent settings version"));
        }
        let locked = override_config.is_some();
        if let Some(config) = override_config {
            state.config = config;
        }
        state.config.validate()?;
        let service = subagent::Service::new(store, state.config.clone())?;
        Ok(Arc::new(Self {
            service,
            path,
            state: Mutex::new(state),
            locked,
            _lease: lease,
        }))
    }
    pub fn view(&self, enabled: bool) -> Result<SettingsView> {
        let saved = self
            .state
            .lock()
            .map_err(|_| error("subagent settings unavailable"))?;
        Ok(SettingsView {
            version: 1,
            revision: saved.revision,
            enabled,
            locked: self.locked,
            restart_required: serde_json::to_value(&saved.config).ok()
                != serde_json::to_value(self.service.config()).ok(),
            active: self.service.config().clone(),
            config: saved.config.clone(),
        })
    }
    pub fn save(&self, revision: u64, config: Config, enabled: bool) -> Result<SettingsView> {
        if self.locked {
            return Err(error("subagent configuration is deployment controlled"));
        }
        config.validate()?;
        let mut saved = self
            .state
            .lock()
            .map_err(|_| error("subagent settings unavailable"))?;
        if saved.revision != revision {
            return Err(error("subagent settings conflict; refresh before saving"));
        }
        let next = Settings {
            version: 1,
            revision: revision
                .checked_add(1)
                .ok_or_else(|| error("subagent revision exhausted"))?,
            config,
        };
        if serde_json::to_vec(&next)
            .map_err(|_| error("subagent settings encoding failed"))?
            .len()
            > 65536
        {
            return Err(error("subagent settings exceed 64 KiB"));
        }
        crate::capabilities::write_state(&self.path, &next)
            .map_err(|_| error("subagent settings save failed; saved state unchanged"))?;
        *saved = next;
        drop(saved);
        self.view(enabled)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_is_revision_guarded_locked_and_reopened_without_reset() {
        let dir = tempfile::tempdir().unwrap();
        let store = sessions::Store::open(&dir.path().join("sessions"), dir.path()).unwrap();
        let path = dir.path().join("settings.json");
        let host = SubagentHost::open(store.clone(), path.clone(), None).unwrap();
        assert!(SubagentHost::open(store.clone(), path.clone(), None).is_err());
        let mut config = host.view(true).unwrap().config;
        config.max_parallel = 2;
        let saved = host.save(0, config.clone(), true).unwrap();
        assert!(saved.restart_required);
        assert_eq!(saved.active.max_parallel, 4);
        assert!(host.save(0, Config::default(), true).is_err());
        let bytes = fs::read(&path).unwrap();
        let mut invalid = config.clone();
        invalid.max_depth = 0;
        assert!(host.save(1, invalid, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(host);
        let host = SubagentHost::open(store.clone(), path.clone(), None).unwrap();
        assert_eq!(host.view(true).unwrap().active.max_parallel, 2);
        drop(host);
        let host =
            SubagentHost::open(store.clone(), path.clone(), Some(Config::default())).unwrap();
        assert!(host.view(true).unwrap().locked);
        assert!(host.save(1, config, true).is_err());
        drop(host);
        fs::write(&path, "broken configuration").unwrap();
        assert!(SubagentHost::open(store, path.clone(), None).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken configuration");
    }
}
pub struct LocalDriver {
    runtime: OnceLock<Weak<dyn AgentRuntime>>,
    #[cfg(feature = "model-management")]
    pub manager: Option<models::ModelManager>,
    #[allow(dead_code)]
    pub planner: bool,
    pub fixed_model: Option<String>,
}
impl LocalDriver {
    pub fn new(
        #[cfg(feature = "model-management")] manager: Option<models::ModelManager>,
        planner: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            runtime: OnceLock::new(),
            #[cfg(feature = "model-management")]
            manager,
            planner,
            fixed_model: std::env::var("AGENT_MODEL_NAME").ok(),
        })
    }
    pub fn bind(&self, runtime: &Arc<dyn AgentRuntime>) -> Result<()> {
        self.runtime
            .set(Arc::downgrade(runtime))
            .map_err(|_| error("subagent driver already bound"))
    }
}
impl Driver for LocalDriver {
    fn prepare(&self, parent: &RunContext, role: &Role) -> Result<Launch> {
        let raw = self
            .runtime
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| error("child runtime is unavailable"))?;
        let mut metadata = parent.metadata.as_ref().clone();
        for key in [
            sessions::SESSION_KEY,
            sessions::TURN_KEY,
            "mona.host_workspace_binding",
            "mona.host_history_session",
            "subagent.root_run",
        ] {
            metadata.remove(key);
        }
        #[cfg(feature = "planner")]
        {
            metadata.remove(planner::PLAN_SEED_KEY);
            if self.planner {
                let mut seed = RunRequest::new("");
                planner::bind_state(&mut seed, &planner::PlanSnapshot::default())?;
                metadata.extend(seed.metadata);
            }
        }
        let inherited = self.fixed_model.as_ref().map(|id| ModelRoute {
            provider_id: "fixed".into(),
            model_id: id.clone(),
        });
        #[cfg(feature = "model-management")]
        if let Some(manager) = &self.manager {
            if let Some(route) = &role.model {
                return Ok(Launch {
                    runtime: manager.runtime_for(
                        raw,
                        models::Selection {
                            provider_id: route.provider_id.clone(),
                            model_id: route.model_id.clone(),
                        },
                    ),
                    model_options: ModelOptions::default(),
                    model: Some(route.clone()),
                    metadata,
                });
            }
            let route = manager.bound_selection(&parent.model_options)?;
            return Ok(Launch {
                runtime: raw,
                model_options: parent.model_options.clone(),
                model: Some(ModelRoute {
                    provider_id: route.provider_id,
                    model_id: route.model_id,
                }),
                metadata,
            });
        }
        if role.model.is_some() {
            return Err(error(
                "a role-specific model requires host model management",
            ));
        }
        Ok(Launch {
            runtime: raw,
            model_options: parent.model_options.clone(),
            model: inherited,
            metadata,
        })
    }
}
