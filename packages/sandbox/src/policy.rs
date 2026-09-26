use crate::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path, PathBuf},
    str::FromStr,
};

/// File effects only: this vocabulary does not promise read or network isolation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
    pub fn confined(self) -> bool {
        self != Self::DangerFullAccess
    }
}
impl FromStr for Mode {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "workspace-write" => Ok(Self::WorkspaceWrite),
            "danger-full-access" => Ok(Self::DangerFullAccess),
            _ => Err(Error::config(
                "sandbox mode must be read-only, workspace-write, or danger-full-access",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub mode: Mode,
    pub workspace_root: PathBuf,
    /// Trusted host identity. It selects private temporary storage, never authorization.
    pub session_id: Option<String>,
}
impl Policy {
    pub fn new(mode: Mode, workspace_root: impl AsRef<Path>) -> Result<Self> {
        let root = workspace_root.as_ref();
        if !root.is_absolute() || !root.is_dir() {
            return Err(Error::config(
                "sandbox workspace must be an existing absolute directory",
            ));
        }
        let root = std::fs::canonicalize(root).map_err(Error::config)?;
        Ok(Self {
            mode,
            workspace_root: root,
            session_id: None,
        })
    }
    pub fn with_session(mut self, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
            return Err(Error::config("invalid sandbox session identity"));
        }
        self.session_id = Some(id);
        Ok(self)
    }
    /// Mirrors DSH's shared filesystem/Seatbelt roots. Process backends have documented
    /// temporary-area differences (bwrap tmpfs, Landlock /tmp, Windows private temp).
    pub fn writable_roots(&self) -> Result<Vec<PathBuf>> {
        if self.mode != Mode::WorkspaceWrite {
            return Ok(Vec::new());
        }
        let mut roots = vec![self.workspace_root.clone()];
        #[cfg(unix)]
        roots.push(PathBuf::from("/tmp"));
        roots.push(std::env::temp_dir());
        let mut canonical = Vec::new();
        for root in roots {
            let root = canonical_target(&root)?;
            if !canonical.contains(&root) {
                canonical.push(root);
            }
        }
        Ok(canonical)
    }
    /// Returns the EXACT canonical destination the trusted mutation must use.
    /// Recheck immediately before mutation. Like DSH this is a path fence, not an
    /// atomic defense against an adversary swapping ancestors during the syscall.
    pub fn check_write(&self, path: &Path) -> Result<PathBuf> {
        if self.mode == Mode::ReadOnly {
            return Err(Error::new(
                ErrorCode::FsSandboxDenied,
                "file mutation denied under read-only mode",
            ));
        }
        let target = canonical_target(path)?;
        if self.mode == Mode::DangerFullAccess {
            return Ok(target);
        }
        if self
            .writable_roots()?
            .iter()
            .any(|root| is_under(&target, root))
        {
            return Ok(target);
        }
        Err(Error::new(
            ErrorCode::FsSandboxDenied,
            "file mutation is outside the workspace and permitted temporary roots",
        ))
    }
}
pub(crate) fn is_under(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        Path::new(&path.to_string_lossy().to_lowercase())
            .starts_with(Path::new(&root.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// Resolve links BEFORE a subsequent `..`, including a not-yet-existing final suffix.
/// Never turn permission failures or dangling links into an apparently allowed path.
pub fn canonical_target(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::config("sandbox target must be absolute"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir => current.push(part.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(name) => {
                current.push(name);
                match std::fs::symlink_metadata(&current) {
                    Ok(_) => {
                        current = std::fs::canonicalize(&current).map_err(Error::config)?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(Error::config(e)),
                }
            }
        }
    }
    Ok(current)
}
