use crate::{
    capture::Capture,
    support::error,
    ToolConfig,
};
use api::{async_trait, ErrorCode, Tool, ToolConcurrency, ToolContext, ToolOutput, ToolSpec};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    time::{self, Instant},
};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483.647;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    PowerShell,
    Bash,
    Sh,
}

#[derive(Clone, Debug)]
pub struct ShellConfig {
    pub executable: PathBuf,
    pub kind: ShellKind,
}
impl ShellConfig {
    pub fn new(executable: impl Into<PathBuf>, kind: ShellKind) -> Self {
        Self {
            executable: executable.into(),
            kind,
        }
    }

    pub fn discover() -> api::Result<Self> {
        #[cfg(windows)]
        {
            if let Some(path) = find_on_path("pwsh.exe") {
                return Ok(Self::new(path, ShellKind::PowerShell));
            }
            if let Some(root) = std::env::var_os("ProgramFiles") {
                let path = PathBuf::from(root).join("PowerShell/7/pwsh.exe");
                if path.is_file() {
                    return Ok(Self::new(path, ShellKind::PowerShell));
                }
            }
            let path = PathBuf::from(
                std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
            )
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            if path.is_file() {
                return Ok(Self::new(path, ShellKind::PowerShell));
            }
            if let Some(path) = find_on_path("powershell.exe") {
                return Ok(Self::new(path, ShellKind::PowerShell));
            }
        }
        #[cfg(not(windows))]
        {
            for (name, kind) in [("bash", ShellKind::Bash), ("sh", ShellKind::Sh)] {
                for root in ["/bin", "/usr/bin"] {
                    let path = Path::new(root).join(name);
                    if path.is_file() {
                        return Ok(Self::new(path, kind));
                    }
                }
                if let Some(path) = find_on_path(name) {
                    return Ok(Self::new(path, kind));
                }
            }
        }
        Err(error(
            ErrorCode::Configuration,
            "no native shell found; configure AGENT_SHELL_PATH",
        ))
    }

    pub fn syntax(&self) -> &'static str {
        match self.kind {
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Bash => "Bash",
            ShellKind::Sh => "POSIX sh",
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|root| root.join(name))
        .find(|path| path.is_file())
}

// Conventional executable names keep existing ToolConfig::new callers usable;
// hosts using a renamed executable can select its syntax with ShellConfig::new.
impl From<PathBuf> for ShellConfig {
    fn from(path: PathBuf) -> Self {
        let kind = match path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "pwsh" | "powershell" => ShellKind::PowerShell,
            "sh" | "dash" => ShellKind::Sh,
            _ => ShellKind::Bash,
        };
        Self::new(path, kind)
    }
}
impl From<&str> for ShellConfig {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}
impl From<&Path> for ShellConfig {
    fn from(path: &Path) -> Self {
        path.to_owned().into()
    }
}
impl From<&PathBuf> for ShellConfig {
    fn from(path: &PathBuf) -> Self {
        path.clone().into()
    }
}
impl From<String> for ShellConfig {
    fn from(path: String) -> Self {
        PathBuf::from(path).into()
    }
}

pub struct ShellTool {
    config: ToolConfig,
}
impl ShellTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    timeout: Option<f64>,
}

/// Also terminates the group if Runtime drops the awaiting future.
struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    armed: bool,
}
impl Drop for ProcessTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.child.start_kill();
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".into(),
            description: format!("Run a command using {} syntax on {} in the configured working directory. Use this syntax, not commands for a different interpreter. Returns combined UTF-8 stdout/stderr and exit status. With the host output archive installed, long output is saved behind an artifact URI for read; otherwise omitted output is explicitly not retained. timeout is in seconds and cannot exceed the host's task/tool deadline.", self.config.shell.syntax(), std::env::consts::OS),
            parameters: json!({"type":"object","properties":{
                "command":{"type":"string"},
                "timeout":{"type":"number","exclusiveMinimum":0,"maximum":MAX_TIMEOUT_SECONDS}
            },"required":["command"],"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        ctx.run.task.check()?;
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid shell arguments"))?;
        let mut capture = Capture::new(&self.config, &ctx)?;
        let timeout = resolve_timeout(args.timeout)?
            .unwrap_or(ctx.run.limits.tool_timeout)
            .min(ctx.run.limits.tool_timeout);
        let deadline = (Instant::now() + timeout).min(ctx.run.task.deadline());
        let task_cancel = ctx.run.task.cancellation();
        let argv = shell_argv(&self.config.shell, &args.command);
        #[cfg(feature = "sandbox")]
        let confinement = if let Some(binding) = &self.config.sandbox {
            let policy = binding.resolve(&ctx.run)?;
            let stop = ctx.run.cancel.child_token(); let _stop_on_drop = stop.clone().drop_guard();
            let plan = tokio::select! {
                biased;
                _ = ctx.run.cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "sandbox preparation cancelled")),
                _ = task_cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "sandbox preparation cancelled")),
                _ = time::sleep_until(deadline) => return Err(error(ErrorCode::Deadline, "sandbox preparation exceeded tool deadline")),
                plan = binding.provider.prepare(&argv, &policy, &stop) => plan.map_err(crate::confinement::failure)?,
            };
            Some((policy.mode, plan))
        } else { None };
        let (program, command_args) = (&argv[0], &argv[1..]);
        #[cfg(feature = "sandbox")]
        let (program, command_args) = confinement.as_ref().map_or((program, command_args), |(_, plan)| (&plan.program, plan.args.as_slice()));
        #[cfg(feature = "sandbox")]
        let mut diagnostics = confinement.as_ref().and_then(|(_, plan)| plan.info).map(|info| sandbox::Diagnostics::new(info.backend));
        let mut command = CommandWrap::with_new(program, |command| {
            command.args(command_args)
                .current_dir(&self.config.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        });
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        {
            let mut flags = process_wrap::tokio::CreationFlags(Default::default());
            flags.0 .0 = 0x0800_0000; // CREATE_NO_WINDOW
            command.wrap(flags).wrap(process_wrap::tokio::JobObject);
        }
        if ctx.run.cancel.is_cancelled() {
            return Err(error(ErrorCode::Cancelled, "shell command cancelled"));
        }
        ctx.run.task.check()?;
        if Instant::now() >= deadline { return Err(error(ErrorCode::Deadline, "shell launch deadline reached")); }
        let mut tree = ProcessTree {
            child: command
                .spawn()
                .map_err(|e| {
                    #[cfg(feature = "sandbox")]
                    if confinement.as_ref().is_some_and(|(_, p)| p.info.is_some()) {
                        return error(ErrorCode::Tool, format!("SANDBOX_UNAVAILABLE: cannot start confined shell: {e}; not retried unconfined"));
                    }
                    error(ErrorCode::Tool, format!("cannot start shell: {e}"))
                })?,
            armed: true,
        };
        let mut stdout = tree
            .child
            .stdout()
            .take()
            .ok_or_else(|| error(ErrorCode::Tool, "stdout capture unavailable"))?;
        let mut stderr = tree
            .child
            .stderr()
            .take()
            .ok_or_else(|| error(ErrorCode::Tool, "stderr capture unavailable"))?;
        let (mut stdout_open, mut stderr_open) = (true, true);
        let mut status = None;
        let mut stdout_buffer = [0; 8192];
        let mut stderr_buffer = [0; 8192];
        let mut failure = None;
        while stdout_open || stderr_open || status.is_none() {
            let (stream, count) = tokio::select! {
                biased;
                _ = ctx.run.cancel.cancelled() => { failure=Some(error(ErrorCode::Cancelled,"shell command cancelled")); break; },
                _ = task_cancel.cancelled() => { failure=Some(error(ErrorCode::Cancelled,"shell task cancelled")); break; },
                _ = time::sleep_until(deadline) => { failure=Some(error(ErrorCode::Deadline,"shell command exceeded its effective deadline")); break; },
                result = tree.child.inner_mut().wait(), if status.is_none() => {
                    match result { Ok(value)=>status=Some(value), Err(e)=>{failure=Some(error(ErrorCode::Tool,format!("cannot wait for shell: {e}")));break;} }
                    // This one-shot tool does not create persistent sessions.
                    let _ = tree.child.start_kill();
                    continue;
                },
                result = stdout.read(&mut stdout_buffer), if stdout_open => {
                    match result { Ok(count)=>{ if count == 0 { stdout_open = false; } (0, count) }, Err(e)=>{failure=Some(error(ErrorCode::Tool,format!("stdout read failed: {e}")));break;} }
                },
                result = stderr.read(&mut stderr_buffer), if stderr_open => {
                    match result { Ok(count)=>{ if count == 0 { stderr_open = false; } (1, count) }, Err(e)=>{failure=Some(error(ErrorCode::Tool,format!("stderr read failed: {e}")));break;} }
                },
            };
            let incoming = if stream == 0 { &stdout_buffer[..count] } else { &stderr_buffer[..count] };
            #[cfg(feature = "sandbox")]
            if stream == 1 { if let Some(diagnostics) = &mut diagnostics { diagnostics.push(incoming); } }
            let appended = tokio::select! {
                biased;
                _ = ctx.run.cancel.cancelled() => Err(error(ErrorCode::Cancelled, "command capture cancelled")),
                _ = task_cancel.cancelled() => Err(error(ErrorCode::Cancelled, "command capture cancelled")),
                _ = time::sleep_until(deadline) => Err(error(ErrorCode::Deadline, "command capture deadline reached")),
                result = capture.append(stream, incoming, count == 0) => result,
            };
            if let Err(e) = appended {
                failure = Some(e);
                break;
            }
        }
        let _ = tree.child.start_kill();
        if matches!(
            time::timeout(Duration::from_secs(2), tree.child.wait()).await,
            Ok(Ok(_))
        ) {
            tree.armed = false;
        }
        if let Some(failure) = failure {
            return Err(failure);
        }
        let status =
            status.ok_or_else(|| error(ErrorCode::Tool, "command exit status unavailable"))?;
        #[cfg(feature = "sandbox")]
        if let Some((mode, plan)) = &confinement {
            let diagnostic = diagnostics.map(|d| d.classify(status.code(), status.success())).flatten();
            let detail = json!({"mode":mode,"backend":plan.info.map(|i| i.backend),"enforcement":plan.info.map(|i| i.enforcement),"diagnostic":diagnostic});
            let _ = ctx.progress.set_detail("sandbox.execution", detail.clone());
            capture.set_sandbox(detail, diagnostic);
        }
        tokio::select! {
            biased;
            _ = ctx.run.cancel.cancelled() => Err(error(ErrorCode::Cancelled, "command archive cancelled")),
            _ = task_cancel.cancelled() => Err(error(ErrorCode::Cancelled, "command archive cancelled")),
            _ = time::sleep_until(deadline) => Err(error(ErrorCode::Deadline, "command archive deadline reached")),
            result = capture.finish(&ctx, status.code(), status.success()) => result,
        }
    }
}

fn shell_argv(shell: &ShellConfig, command: &str) -> Vec<std::ffi::OsString> {
    let mut argv = vec![shell.executable.clone().into_os_string()];
    match shell.kind {
        ShellKind::PowerShell => {
            let script = format!("$ProgressPreference = 'SilentlyContinue'; if ($ExecutionContext.SessionState.LanguageMode -eq 'FullLanguage') {{ [Console]::InputEncoding = [Console]::OutputEncoding = $OutputEncoding = [System.Text.UTF8Encoding]::new($false) }}; $ErrorActionPreference = 'Stop'; & {{ {command} }}; $monaCommandSucceeded = $?; if ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }}; if (-not $monaCommandSucceeded) {{ exit 1 }}");
            use base64::Engine;
            let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
            argv.extend(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-OutputFormat", "Text", "-EncodedCommand"].map(Into::into));
            argv.push(base64::engine::general_purpose::STANDARD.encode(bytes).into());
        },
        ShellKind::Bash | ShellKind::Sh => argv.extend(["-c".into(), command.into()]),
    }
    argv
}

fn resolve_timeout(value: Option<f64>) -> api::Result<Option<Duration>> {
    match value {
        None => Ok(None),
        Some(seconds) if seconds.is_finite() && seconds > 0.0 && seconds <= MAX_TIMEOUT_SECONDS => {
            Ok(Some(Duration::from_secs_f64(seconds)))
        }
        _ => Err(error(
            ErrorCode::Schema,
            "timeout must be a finite positive number of seconds within the supported range",
        )),
    }
}
