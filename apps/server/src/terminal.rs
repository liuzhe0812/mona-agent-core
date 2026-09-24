//! User-driven PTY sessions. Separate from Agent tools; no model execution or shared process cwd.
use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use workspace::{error, Result};
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_TERMINALS: usize = 8;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: String,
    pub id: String,
}
struct Output {
    bytes: VecDeque<u8>,
    start: u64,
    end: u64,
    exited: bool,
    code: Option<u32>,
}
struct Input {
    writer: Box<dyn Write + Send>,
    last: u64,
    data: String,
    faulted: bool,
}
struct Terminal {
    target: Target,
    output: Mutex<Output>,
    input: Mutex<Input>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    touched: Mutex<Instant>,
}
impl Terminal {
    fn kill(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }
}
#[derive(Default)]
pub struct Terminals {
    entries: Mutex<HashMap<String, Arc<Terminal>>>,
}
#[derive(Serialize)]
pub struct Page {
    pub base64: String,
    pub next: u64,
    pub dropped: bool,
    pub exited: bool,
    pub exit_code: Option<u32>,
}
fn internal(_: impl std::fmt::Display) -> workspace::Error {
    error("io", "终端操作失败，请关闭此终端后重开。")
}
pub fn valid_key(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn size(rows: u16, cols: u16) -> Result<PtySize> {
    if !(2..=300).contains(&rows) || !(2..=500).contains(&cols) {
        return Err(error("invalid_request", "终端尺寸超限。"));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
impl Terminals {
    fn get(&self, target: &Target, id: &str) -> Result<Arc<Terminal>> {
        let terminal = self
            .entries
            .lock()
            .map_err(internal)?
            .get(id)
            .cloned()
            .ok_or_else(|| error("not_found", "终端已关闭或宿主已重启。"))?;
        if &terminal.target != target {
            return Err(error("forbidden", "终端不属于此工作区。"));
        }
        *terminal.touched.lock().map_err(internal)? = Instant::now();
        Ok(terminal)
    }
    pub fn enabled() -> bool {
        std::env::var("AGENT_TERMINAL").as_deref() != Ok("0")
    }
    pub fn open(
        &self,
        target: Target,
        id: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<String> {
        if !Self::enabled() {
            return Err(error("unsupported", "部署未开放交互终端。"));
        }
        if !valid_key(id) {
            return Err(error("invalid_request", "终端请求标识无效。"));
        }
        let geometry = size(rows, cols)?;
        let mut entries = self.entries.lock().map_err(internal)?;
        if let Some(existing) = entries.get(id) {
            if existing.target == target {
                return Ok(id.into());
            }
            return Err(error("conflict", "终端创建标识不能更改工作区。"));
        }
        if entries.len() >= MAX_TERMINALS {
            return Err(error(
                "capacity",
                "最多打开 8 个终端，请先关闭不再使用的终端。",
            ));
        }
        let shell = crate::tool_setup::shell_from_environment().map_err(internal)?;
        let pair = native_pty_system().openpty(geometry).map_err(internal)?;
        let mut command = CommandBuilder::new(&shell.executable);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        if shell.kind == tools::ShellKind::PowerShell {
            command.args(["-NoLogo", "-NoProfile"]);
        }
        // Never inherit host model/bearer credentials into a user-facing shell by accident.
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("AGENT_") || name.starts_with("MONA_") {
                command.env_remove(&key);
            }
        }
        let reader = pair.master.try_clone_reader().map_err(internal)?;
        let writer = pair.master.take_writer().map_err(internal)?;
        let mut child = pair.slave.spawn_command(command).map_err(internal)?;
        let killer = child.clone_killer();
        drop(pair.slave);
        let terminal = Arc::new(Terminal {
            target,
            output: Mutex::new(Output {
                bytes: VecDeque::new(),
                start: 0,
                end: 0,
                exited: false,
                code: None,
            }),
            input: Mutex::new(Input {
                writer,
                last: 0,
                data: String::new(),
                faulted: false,
            }),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            touched: Mutex::new(Instant::now()),
        });
        let output = terminal.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let Ok(mut state) = output.output.lock() else {
                    break;
                };
                state.bytes.extend(&buffer[..count]);
                state.end += count as u64;
                if state.bytes.len() > MAX_OUTPUT {
                    let discard = state.bytes.len() - MAX_OUTPUT;
                    state.bytes.drain(..discard);
                    state.start += discard as u64;
                }
            }
            let code = child.wait().ok().map(|s| s.exit_code());
            if let Ok(mut state) = output.output.lock() {
                state.exited = true;
                state.code = code;
            }
        });
        entries.insert(id.into(), terminal);
        Ok(id.into())
    }
    pub fn output(&self, target: &Target, id: &str, after: u64) -> Result<Page> {
        let terminal = self.get(target, id)?;
        let state = terminal.output.lock().map_err(internal)?;
        if after > state.end {
            return Err(error("conflict", "终端输出游标无效。"));
        }
        let start = after.max(state.start);
        let bytes: Vec<u8> = state
            .bytes
            .iter()
            .skip((start - state.start) as usize)
            .take(65536)
            .copied()
            .collect();
        Ok(Page {
            next: start + bytes.len() as u64,
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            dropped: after < state.start,
            exited: state.exited && start + 65536 >= state.end,
            exit_code: state.code,
        })
    }
    pub fn input(&self, target: &Target, id: &str, sequence: u64, data: &str) -> Result<()> {
        if data.len() > 16384 || sequence == 0 {
            return Err(error("invalid_request", "终端输入超限。"));
        }
        let terminal = self.get(target, id)?;
        if terminal.output.lock().map_err(internal)?.exited {
            return Err(error("conflict", "终端进程已经退出。"));
        }
        let mut input = terminal.input.lock().map_err(internal)?;
        if input.faulted {
            return Err(error(
                "conflict",
                "之前的终端输入结果未确认，请关闭终端后重新打开。",
            ));
        }
        if input.last == sequence && input.data == data {
            return Ok(());
        }
        if sequence != input.last + 1 {
            return Err(error("conflict", "终端输入顺序不一致，不会自动重放。"));
        }
        // Reserve before I/O: an ambiguous write must never be replayed.
        input.last = sequence;
        input.data = data.into();
        match input
            .writer
            .write_all(data.as_bytes())
            .and_then(|_| input.writer.flush())
        {
            Ok(()) => Ok(()),
            Err(e) => {
                input.faulted = true;
                Err(internal(e))
            }
        }
    }
    pub fn resize(&self, target: &Target, id: &str, rows: u16, cols: u16) -> Result<()> {
        let terminal = self.get(target, id)?;
        terminal
            .master
            .lock()
            .map_err(internal)?
            .resize(size(rows, cols)?)
            .map_err(internal)?;
        Ok(())
    }
    pub fn close(&self, target: &Target, id: &str) -> Result<()> {
        let mut entries = self.entries.lock().map_err(internal)?;
        if let Some(terminal) = entries.get(id) {
            if &terminal.target != target {
                return Err(error("forbidden", "终端不属于此工作区。"));
            }
        }
        if let Some(terminal) = entries.remove(id) {
            terminal.kill();
        }
        Ok(())
    }
    pub fn sweep(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, terminal| {
                let stale = terminal
                    .touched
                    .lock()
                    .map(|t| t.elapsed() > Duration::from_secs(1800))
                    .unwrap_or(true);
                if stale {
                    terminal.kill();
                }
                !stale
            });
        }
    }
    pub fn shutdown(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            for (_, terminal) in entries.drain() {
                terminal.kill();
            }
        }
    }
}
impl Drop for Terminals {
    fn drop(&mut self) {
        self.shutdown();
    }
}
