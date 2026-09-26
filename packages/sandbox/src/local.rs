#[cfg(any(windows, target_os = "linux"))]
use crate::Mode;
use crate::{check_cancel, profiles, CancellationToken, Error, ErrorCode, Policy, Result};
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(windows)]
use std::{collections::BTreeMap, sync::Arc};
#[cfg(target_os = "linux")]
use std::{ffi::OsStr, process::Stdio, time::Duration};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "linux")]
use tokio::process::Command;
use tokio::sync::OnceCell;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Bubblewrap,
    Landlock,
    Seatbelt,
    WindowsAcl,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforcement {
    Full,
    Partial,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BackendInfo {
    pub backend: Backend,
    pub enforcement: Enforcement,
}

/// A trusted native executable, not a model-controlled command or remotely loaded plugin.
#[derive(Clone, Debug)]
pub struct Runner {
    program: PathBuf,
    prefix: Vec<OsString>,
}
impl Runner {
    pub fn standalone(path: impl Into<PathBuf>) -> Self {
        Self {
            program: path.into(),
            prefix: Vec::new(),
        }
    }
    /// The executable must call runner::dispatch BEFORE starting threads or loading the host.
    pub fn embedded(path: impl Into<PathBuf>) -> Self {
        Self {
            program: path.into(),
            prefix: vec!["--mona-sandbox".into()],
        }
    }
    fn argv(&self) -> Vec<OsString> {
        let mut argv = vec![self.program.clone().into_os_string()];
        argv.extend(self.prefix.clone());
        argv
    }
}

/// Keep this value alive until the entire process tree is terminated, not merely until spawn.
pub struct CommandPlan {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub info: Option<BackendInfo>,
    #[cfg(windows)]
    _temp: Option<Arc<crate::windows::PrivateTemp>>,
}
impl CommandPlan {
    fn new(argv: Vec<OsString>, info: Option<BackendInfo>) -> Result<Self> {
        if argv.is_empty() || argv[0].is_empty() {
            return Err(Error::config("sandbox command argv is empty"));
        }
        Ok(Self {
            program: argv[0].clone(),
            args: argv[1..].to_vec(),
            info,
            #[cfg(windows)]
            _temp: None,
        })
    }
}

/// Host-lifetime provider. Linux probes choose a runner once; Windows caches private
/// temporary areas per session/workspace and leaves only the intentional workspace ACL.
/// No Agent, database, UI or plugin framework dependency.
pub struct LocalSandbox {
    runner: Runner,
    closed: AtomicBool,
    backend: OnceCell<Result<BackendInfo>>,
    #[cfg(windows)]
    temps: tokio::sync::Mutex<BTreeMap<(String, PathBuf), Arc<crate::windows::PrivateTemp>>>,
}
impl LocalSandbox {
    pub fn new(runner: Runner) -> Self {
        Self {
            runner,
            closed: AtomicBool::new(false),
            backend: OnceCell::new(),
            #[cfg(windows)]
            temps: Default::default(),
        }
    }
    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(Error::unavailable("sandbox provider is closed"))
        } else {
            Ok(())
        }
    }
    pub async fn backend(&self, cancel: &CancellationToken) -> Result<BackendInfo> {
        self.check_open()?;
        check_cancel(cancel)?;
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::new(ErrorCode::SandboxCancelled, "sandbox selection cancelled")),
            result = self.backend.get_or_init(|| self.select()) => result,
        };
        result.clone()
    }
    async fn select(&self) -> Result<BackendInfo> {
        #[cfg(target_os = "linux")]
        {
            let policy = Policy::new(Mode::ReadOnly, "/")?;
            let mut args = profiles::bubblewrap(&policy);
            args.extend(["--".into(), "/bin/true".into()]);
            if probe(OsStr::new("bwrap"), &args).await == Some(0) {
                return Ok(BackendInfo {
                    backend: Backend::Bubblewrap,
                    enforcement: Enforcement::Full,
                });
            }
            let mut args = self.runner.prefix.clone();
            args.extend(["landlock".into(), "--probe".into()]);
            let enforcement = match probe(self.runner.program.as_os_str(), &args).await {
                Some(0) => Enforcement::Full, Some(10) => Enforcement::Partial,
                _ => return Err(Error::unavailable("neither Bubblewrap nor the native Landlock runner is usable; refusing unconfined execution")),
            };
            Ok(BackendInfo {
                backend: Backend::Landlock,
                enforcement,
            })
        }
        #[cfg(target_os = "macos")]
        {
            if !Path::new("/usr/bin/sandbox-exec").is_file() {
                return Err(Error::unavailable("sandbox-exec is missing"));
            }
            Ok(BackendInfo {
                backend: Backend::Seatbelt,
                enforcement: Enforcement::Full,
            })
        }
        #[cfg(windows)]
        {
            if !self.runner.program.is_file() {
                return Err(Error::unavailable(
                    "native Windows sandbox runner is missing",
                ));
            }
            Ok(BackendInfo {
                backend: Backend::WindowsAcl,
                enforcement: Enforcement::Partial,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        Err(Error::unavailable(
            "no local sandbox backend on this platform",
        ))
    }
    pub async fn prepare(
        &self,
        argv: &[OsString],
        policy: &Policy,
        cancel: &CancellationToken,
    ) -> Result<CommandPlan> {
        self.check_open()?;
        check_cancel(cancel)?;
        if argv.is_empty()
            || argv.len() > 1024
            || argv.iter().any(|s| s.to_string_lossy().contains('\0'))
        {
            return Err(Error::config("invalid command argv"));
        }
        let mut checked = Policy::new(policy.mode, &policy.workspace_root)?;
        if let Some(id) = &policy.session_id {
            checked = checked.with_session(id)?;
        }
        if !checked.mode.confined() {
            return CommandPlan::new(argv.to_vec(), None);
        }
        let info = self.backend(cancel).await?;
        self.check_open()?;
        let mut wrapped = match info.backend {
            Backend::Bubblewrap => {
                let mut a = vec![OsString::from("bwrap")];
                a.extend(profiles::bubblewrap(&checked));
                a
            }
            Backend::Seatbelt => vec![
                "/usr/bin/sandbox-exec".into(),
                "-p".into(),
                profiles::seatbelt(&checked)?.into(),
            ],
            Backend::Landlock => {
                let mut a = self.runner.argv();
                a.extend([
                    "landlock".into(),
                    "--mode".into(),
                    checked.mode.as_str().into(),
                    "--workspace".into(),
                    checked.workspace_root.clone().into_os_string(),
                ]);
                a
            }
            Backend::WindowsAcl => {
                #[cfg(windows)]
                {
                    let temp = if checked.mode == Mode::WorkspaceWrite {
                        Some(self.private_temp(&checked, cancel).await?)
                    } else {
                        None
                    };
                    check_cancel(cancel)?;
                    let mut a = self.runner.argv();
                    a.extend([
                        "windows-acl".into(),
                        "--mode".into(),
                        checked.mode.as_str().into(),
                        "--workspace".into(),
                        checked.workspace_root.clone().into_os_string(),
                    ]);
                    if let Some(temp) = &temp {
                        a.extend(["--temp".into(), temp.path().as_os_str().to_owned()]);
                    }
                    a.push("--".into());
                    a.extend_from_slice(argv);
                    let mut plan = CommandPlan::new(a, Some(info))?;
                    plan._temp = temp;
                    return Ok(plan);
                }
                #[cfg(not(windows))]
                return Err(Error::unavailable("Windows backend on another platform"));
            }
        };
        wrapped.push("--".into());
        wrapped.extend_from_slice(argv);
        check_cancel(cancel)?;
        CommandPlan::new(wrapped, Some(info))
    }
    #[cfg(windows)]
    async fn private_temp(
        &self,
        policy: &Policy,
        cancel: &CancellationToken,
    ) -> Result<Arc<crate::windows::PrivateTemp>> {
        // The blocking preparation owns its RAII temp even if the caller is cancelled.
        // Cached session identities keep temp grants reusable but bounded.
        let mut temps = tokio::select! { biased; _ = cancel.cancelled() => return Err(Error::new(ErrorCode::SandboxCancelled, "sandbox grant wait cancelled")), guard = self.temps.lock() => guard };
        self.check_open()?;
        let key = policy
            .session_id
            .as_ref()
            .map(|id| (id.clone(), policy.workspace_root.clone()));
        if let Some(key) = &key {
            if let Some(temp) = temps.get(key) {
                return Ok(temp.clone());
            }
        }
        if key.is_some() && temps.len() >= 1024 {
            return Err(Error::new(
                ErrorCode::SandboxCapacity,
                "sandbox session capacity reached",
            ));
        }
        let root = policy.workspace_root.clone();
        let stop = cancel.clone();
        let temp =
            tokio::task::spawn_blocking(move || crate::windows::PrivateTemp::create(&root, &stop))
                .await
                .map_err(Error::unavailable)??;
        check_cancel(cancel)?;
        self.check_open()?;
        let temp = Arc::new(temp);
        if let Some(key) = key {
            temps.insert(key, temp.clone());
        }
        Ok(temp)
    }
    /// Release a deleted/closed session's cached temporary grants, never another session's.
    /// The host must first stop this session's commands; a live lease refuses cleanup.
    pub async fn release_session(&self, session_id: &str) -> Result<()> {
        #[cfg(windows)]
        {
            let mut temps = self.temps.lock().await;
            if temps
                .iter()
                .any(|((id, _), temp)| id == session_id && Arc::strong_count(temp) != 1)
            {
                return Err(Error::config(
                    "session still has live sandbox command leases",
                ));
            }
            let keys: Vec<_> = temps
                .keys()
                .filter(|(id, _)| id == session_id)
                .cloned()
                .collect();
            let owned = keys
                .into_iter()
                .filter_map(|key| temps.remove(&key))
                .collect();
            drop(temps);
            close_temps(owned).await?;
        }
        #[cfg(not(windows))]
        let _ = session_id;
        Ok(())
    }
    /// Release cached temporary capabilities only after host tasks have stopped.
    /// Standing workspace ACLs intentionally remain, as in DSH.
    pub async fn shutdown(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        #[cfg(windows)]
        {
            let mut temps = self.temps.lock().await;
            if temps.values().any(|t| Arc::strong_count(t) != 1) {
                return Err(Error::config("sandbox still has live command leases"));
            }
            let owned = std::mem::take(&mut *temps).into_values().collect();
            drop(temps);
            close_temps(owned).await?;
        }
        Ok(())
    }
}

#[cfg(windows)]
async fn close_temps(owned: Vec<Arc<crate::windows::PrivateTemp>>) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut errors = Vec::new();
        for temp in owned {
            match Arc::try_unwrap(temp) {
                Ok(temp) => {
                    if let Err(e) = temp.close() {
                        errors.push(e.to_string());
                    }
                }
                Err(_) => errors.push("sandbox lease unexpectedly retained".into()),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::unavailable(errors.join("; ")))
        }
    })
    .await
    .map_err(Error::unavailable)?
}

#[cfg(target_os = "linux")]
async fn probe(program: &OsStr, args: &[OsString]) -> Option<i32> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => status.code(),
        _ => {
            let _ = child.kill().await;
            None
        }
    }
}
