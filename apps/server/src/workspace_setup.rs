//! Host policy and durable default-directory allocation. Independent of the optional projects package.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use workspace::{canonical_root, error, path_text, Result};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub revision: u64,
    pub default_root: String,
}
#[derive(Serialize)]
pub struct View {
    pub revision: u64,
    pub default_root: String,
    pub locked: bool,
    pub projects_enabled: bool,
}
pub struct WorkspaceSettings {
    path: PathBuf,
    state: Mutex<Settings>,
    locked: bool,
    private: Vec<PathBuf>,
    _lease: File,
}
fn io(_: impl std::fmt::Display) -> workspace::Error {
    error("io", "工作区配置无法读写；原配置保持不变。")
}
pub fn state_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MONA_DEV_STATE_DIR").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| error("invalid_request", "请设置宿主私有状态目录。"))?;
    Ok(root.join("mona-agent-core"))
}
pub fn session_base(demo: bool) -> Result<PathBuf> {
    match std::env::var_os("AGENT_SESSIONS_DIR") {
        Some(p) if !p.is_empty() => Ok(PathBuf::from(p)),
        Some(_) => Err(error("invalid_request", "AGENT_SESSIONS_DIR 不能为空。")),
        None => Ok(state_root()?.join(if demo { "demo-sessions" } else { "sessions" })),
    }
}
impl WorkspaceSettings {
    pub fn from_environment(demo: bool) -> Result<Arc<Self>> {
        let state = state_root()?;
        fs::create_dir_all(&state).map_err(io)?;
        let path = std::env::var_os("AGENT_WORKSPACE_SETTINGS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                state.join(if demo {
                    "demo-workspace-settings.json"
                } else {
                    "workspace-settings.json"
                })
            });
        let configured = std::env::var_os("AGENT_WORKSPACE_DIR").map(PathBuf::from);
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or_else(|| error("invalid_request", "请指定默认工作目录。"))?;
        let initial = home.join("Mona/workspaces");
        let mut private = vec![
            fs::canonicalize(&state).map_err(io)?,
            session_base(demo)?,
            path.clone(),
            path.with_extension("lock"),
        ];
        for name in [
            "AGENT_SPILL_DIR",
            "AGENT_MEMORY_DIR",
            "AGENT_MODEL_SETTINGS_PATH",
            "AGENT_CAPABILITY_STATE_PATH",
            "AGENT_PROJECTS_PATH",
        ] {
            if let Some(value) = std::env::var_os(name) {
                private.push(PathBuf::from(value));
            }
        }
        private.push(state.join("spill"));
        Self::open(path, initial, configured, private).map(Arc::new)
    }
    pub fn open(
        path: PathBuf,
        initial: PathBuf,
        override_root: Option<PathBuf>,
        private: Vec<PathBuf>,
    ) -> Result<Self> {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| error("invalid_request", "状态路径无效。"))?,
        )
        .map_err(io)?;
        for p in [&path, &path.with_extension("lock")] {
            if let Ok(m) = fs::symlink_metadata(p) {
                if workspace::is_link(&m) || !m.is_file() {
                    return Err(error("forbidden", "状态文件不能是链接。"));
                }
            }
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(io)?;
        lease
            .try_lock()
            .map_err(|_| error("conflict", "工作区配置已由其他宿主占用。"))?;
        let saved = match File::open(&path) {
            Ok(f) => {
                let mut b = Vec::new();
                f.take(32769).read_to_end(&mut b).map_err(io)?;
                if b.len() > 32768 {
                    return Err(error("capacity", "工作区配置过大。"));
                }
                let v: Settings = serde_json::from_slice(&b)
                    .map_err(|_| error("invalid_request", "工作区配置损坏，未重置。"))?;
                if v.version != 1 || !Path::new(&v.default_root).is_absolute() {
                    return Err(error("invalid_request", "不支持的工作区配置格式。"));
                }
                Some(v)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(e)),
        };
        let locked = override_root.is_some();
        let mut value = saved.unwrap_or(Settings {
            version: 1,
            revision: 0,
            default_root: path_text(&initial)?,
        });
        if let Some(root) = override_root {
            value.default_root = path_text(&canonical_root(&root)?)?;
        } else if value.revision == 0 && !path.exists() {
            fs::create_dir_all(&initial).map_err(io)?;
            value.default_root = path_text(&canonical_root(&initial)?)?;
        }
        let this = Self {
            path,
            state: Mutex::new(value),
            locked,
            private,
            _lease: lease,
        };
        // Keep unavailable saved defaults visible and editable; creation still fails closed.
        // Explicit deployment overrides and first-run defaults were validated above.
        if Path::new(&this.view(false)?.default_root).exists() {
            this.allowed_root(Path::new(&this.view(false)?.default_root))?;
        }
        Ok(this)
    }
    pub fn view(&self, projects_enabled: bool) -> Result<View> {
        let s = self.state.lock().map_err(io)?;
        Ok(View {
            revision: s.revision,
            default_root: s.default_root.clone(),
            locked: self.locked,
            projects_enabled,
        })
    }
    pub fn private_paths(&self) -> Vec<PathBuf> {
        self.private.clone()
    }
    pub fn allowed_root(&self, path: &Path) -> Result<PathBuf> {
        let root = canonical_root(path)?;
        for p in &self.private {
            let p = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if workspace::contains_path(&p, &root) {
                return Err(error("forbidden", "不能将宿主私有状态目录作为工作目录。"));
            }
        }
        Ok(root)
    }
    pub fn update(&self, revision: u64, root: &Path) -> Result<View> {
        if self.locked {
            return Err(error("forbidden", "默认目录由部署配置锁定。"));
        }
        let root = self.allowed_root(root)?;
        let mut current = self.state.lock().map_err(io)?;
        if current.revision != revision {
            return Err(error("conflict", "工作区设置已更新，请刷新。"));
        }
        let next = Settings {
            version: 1,
            revision: current
                .revision
                .checked_add(1)
                .ok_or_else(|| error("capacity", "配置版本超限。"))?,
            default_root: path_text(&root)?,
        };
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.path.parent().unwrap()).map_err(io)?;
        serde_json::to_writer(temporary.as_file_mut(), &next).map_err(io)?;
        temporary.flush().map_err(io)?;
        temporary.as_file().sync_all().map_err(io)?;
        temporary
            .persist(&self.path)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        #[cfg(unix)]
        File::open(self.path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        *current = next;
        Ok(View {
            revision: current.revision,
            default_root: current.default_root.clone(),
            locked: false,
            projects_enabled: false,
        })
    }
    /// Caller supplies explicit project/embedding roots only after authorization. No project dependency here.
    pub fn create_session(
        &self,
        store: &sessions::Store,
        key: &str,
        explicit: Option<&Path>,
        metadata: BTreeMap<String, String>,
    ) -> Result<sessions::Header> {
        if key.is_empty()
            || key.len() > 62
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(error("invalid_request", "会话创建请求 ID 无效。"));
        }
        let id = format!("s-{key}");
        let state = self.state.lock().map_err(io)?;
        match store.header(&id) {
            Ok(header) => {
                if header.metadata != metadata {
                    return Err(error("conflict", "同一创建请求不能改变项目归属。"));
                }
                if let Some(path) = explicit {
                    if !workspace::same_path(
                        Path::new(&header.workspace),
                        &self.allowed_root(path)?,
                    ) {
                        return Err(error("conflict", "同一创建请求不能改变目录。"));
                    }
                }
                return Ok(header);
            }
            Err(e) if e.code == sessions::SessionErrorCode::NotFound => {}
            Err(e) => return Err(error("io", &e.message)),
        }
        let (root, allocated) = if let Some(path) = explicit {
            (self.allowed_root(path)?, false)
        } else {
            let root = self.allowed_root(Path::new(&state.default_root))?.join(&id);
            fs::create_dir(&root).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    error(
                        "conflict",
                        "会话目录已存在，未接管或覆盖其中内容。请刷新会话列表。",
                    )
                } else {
                    io(e)
                }
            })?;
            (canonical_root(&root)?, true)
        };
        match store.create_in(key, &root, metadata) {
            Ok(h) => Ok(h),
            Err(e) => {
                // A failed sync can follow a committed session header. Never remove its cwd.
                // Only an authoritative absence permits cleanup of our own still-empty directory.
                if allocated
                    && matches!(store.header(&id), Err(ref status) if status.code == sessions::SessionErrorCode::NotFound)
                {
                    let _ = fs::remove_dir(&root);
                }
                Err(error("io", &e.message))
            }
        }
    }
    pub fn bound_root(&self, path: &Path) -> Result<PathBuf> {
        let root = self.allowed_root(path)?;
        if !workspace::same_path(&root, path) {
            return Err(error(
                "conflict",
                "会话原工作目录的链接目标已改变，请确认目录后再继续。",
            ));
        }
        Ok(root)
    }
    pub fn files(&self, header: &sessions::Header) -> Result<workspace::Directory> {
        let root = self.bound_root(Path::new(&header.workspace))?;
        Ok(workspace::Directory::open(&root)?.excluding(self.private_paths()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_sessions_are_distinct_and_root_changes_never_rebind_or_delete_files() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("state");
        let root = tmp.path().join("root");
        let next = tmp.path().join("next");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir(&next).unwrap();
        let path = base.join("workspace.json");
        let store = sessions::Store::open(&base.join("sessions"), &root).unwrap();
        let settings =
            WorkspaceSettings::open(path.clone(), root.clone(), None, vec![base.clone()]).unwrap();
        let a = settings
            .create_session(&store, "a", None, BTreeMap::new())
            .unwrap();
        let b = settings
            .create_session(&store, "b", None, BTreeMap::new())
            .unwrap();
        assert_ne!(a.workspace, b.workspace);
        assert_eq!(Path::new(&a.workspace).file_name().unwrap(), "s-a");
        fs::write(Path::new(&a.workspace).join("keep.txt"), "saved").unwrap();
        let selected = settings.update(0, &next).unwrap();
        assert_eq!(selected.revision, 1);
        assert!(settings.update(0, &root).is_err());
        let retry = settings
            .create_session(&store, "a", None, BTreeMap::new())
            .unwrap();
        assert_eq!(retry.workspace, a.workspace);
        let c = settings
            .create_session(&store, "c", None, BTreeMap::new())
            .unwrap();
        assert!(Path::new(&c.workspace).starts_with(canonical_root(&next).unwrap()));
        let renamed = store.rename(&a.id, a.revision, "rename").unwrap();
        assert_eq!(renamed.workspace, a.workspace);
        store.delete(&a.id, renamed.revision).unwrap();
        assert!(Path::new(&a.workspace).join("keep.txt").exists());
        assert!(settings
            .create_session(&store, "a", None, BTreeMap::new())
            .is_ok()); // New default may allocate a different directory; no old files removed.
        assert!(settings.allowed_root(&base).is_err());
        assert!(settings.update(1, &tmp.path().join("absent")).is_err());
        drop(settings);
        drop(store);
        let settings =
            WorkspaceSettings::open(path, root.clone(), None, vec![base.clone()]).unwrap();
        let store = sessions::Store::open(&base.join("sessions"), &next).unwrap();
        assert_eq!(store.header(&b.id).unwrap().workspace, b.workspace);
        assert_eq!(settings.view(false).unwrap().revision, 1);
    }
    #[test]
    fn explicit_directories_need_no_project_package_and_saved_missing_paths_are_not_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let root = tmp.path().join("default");
        let project = tmp.path().join("explicit");
        fs::create_dir(&project).unwrap();
        let config = state.join("workspace.json");
        let settings =
            WorkspaceSettings::open(config.clone(), root.clone(), None, vec![state.clone()])
                .unwrap();
        let store = sessions::Store::open(&state.join("sessions"), &root).unwrap();
        let h = settings
            .create_session(&store, "explicit", Some(&project), BTreeMap::new())
            .unwrap();
        assert_eq!(
            h.workspace,
            path_text(&canonical_root(&project).unwrap()).unwrap()
        );
        settings.update(0, &project).unwrap();
        drop(settings);
        fs::remove_dir(&project).unwrap();
        let settings = WorkspaceSettings::open(config, root, None, vec![state]).unwrap();
        assert!(settings.files(&h).is_err());
        assert!(settings
            .create_session(&store, "new", None, BTreeMap::new())
            .is_err());
        assert_eq!(store.header(&h.id).unwrap().workspace, h.workspace);
    }
}
