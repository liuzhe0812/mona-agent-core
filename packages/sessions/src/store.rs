//! Local session storage. Hosts supply paths; commits are atomic full snapshots, not UI events.
#[path = "context.rs"]
mod context;
#[cfg(feature = "search")]
#[path = "search.rs"]
pub mod search;
use crate::{SessionError, SessionErrorCode as Code, SessionResult as Result};
use api::*;
use context::RunBase;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const SESSION_KEY: &str = "mona.session_id";
pub const TURN_KEY: &str = "mona.turn_id";
const FORMAT: u32 = 3;
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_SESSIONS: usize = 1000;
const MAX_TURNS: usize = 512;

pub(crate) fn error(code: Code, message: &str) -> SessionError {
    SessionError::new(code, message)
}
fn io_error(_: impl std::fmt::Display) -> SessionError {
    error(
        Code::Internal,
        "会话存储读写失败；原记录未被当作空会话，请检查目录权限和磁盘空间。",
    )
}
fn corrupt() -> SessionError {
    error(
        Code::Internal,
        "会话文件损坏或格式不受支持；已拒绝继续，未覆盖原文件。",
    )
}
pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(error(
            Code::InvalidRequest,
            "会话或请求 ID 必须是 1–64 位字母、数字、横线或下划线。",
        ));
    }
    Ok(())
}
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Limited,
    Interrupted,
}
impl Status {
    pub fn from_run(status: RunStatus) -> Self {
        match status {
            RunStatus::Completed => Self::Completed,
            RunStatus::Failed => Self::Failed,
            RunStatus::Cancelled => Self::Cancelled,
            RunStatus::TimedOut => Self::TimedOut,
            RunStatus::Limited => Self::Limited,
        }
    }
    pub fn outcome(self) -> RunStatus {
        match self {
            Self::Completed => RunStatus::Completed,
            Self::Cancelled => RunStatus::Cancelled,
            Self::TimedOut => RunStatus::TimedOut,
            Self::Limited => RunStatus::Limited,
            _ => RunStatus::Failed,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: u32,
    pub id: String,
    pub workspace: String,
    /// Host-owned navigation metadata; never used as directory authority or model input.
    pub metadata: std::collections::BTreeMap<String, String>,
    pub title: String,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub turn_count: usize,
    pub status: Status,
    // Saved session metadata; interpretation/presentation belongs to the host.
    pub pinned: bool,
    pub archived: bool,
    pub unread: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub id: String,
    pub run_id: Option<String>,
    pub prompt: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub status: Status,
    pub start: usize,
    pub end: usize,
    pub usage: TaskUsage,
    pub error: Option<AgentError>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub turns: Vec<Turn>,
    // The exact latest immutable API checkpoint; no parallel persisted-message vocabulary.
    // Earlier facts live in archive; base maps the current working prefix back to those facts.
    pub checkpoint: Option<RunCheckpoint>,
    pub checkpoint_turn: Option<usize>,
    pub archive: Vec<Message>,
    base: Option<RunBase>,
    pub compaction: Option<serde_json::Value>,
}
pub struct Document {
    pub header: Header,
    pub body: Body,
}

pub enum Prepared {
    Existing(Turn, Header),
    New { history: Vec<Message> },
}
#[derive(Serialize)]
pub struct Listing {
    pub sessions: Vec<Header>,
    pub next_offset: Option<usize>,
    pub unreadable: usize,
}

pub struct Store {
    root: PathBuf,
    workspace: String,
    gate: Mutex<()>,
    // OS-owned lock is released even on process death. Never unlink a held lock file.
    _lease: File,
    #[cfg(feature = "compaction")]
    compactor: Mutex<Option<compaction::Compactor>>,
}
impl Store {
    pub fn open(base: &Path, workspace: &Path) -> Result<Arc<Self>> {
        if !workspace.is_absolute() {
            return Err(error(Code::InvalidRequest, "默认工作目录必须是绝对路径。"));
        }
        let workspace = workspace.to_str().ok_or_else(corrupt)?.to_owned();
        // The host chooses the authority namespace. Changing the default cwd must not hide history.
        let root = base.to_owned();
        fs::create_dir_all(&root).map_err(io_error)?;
        regular_path(&root, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        }
        let lock_path = root.join("writer.lock");
        if lock_path.exists() {
            regular_path(&lock_path, false)?;
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(io_error)?;
        lease.try_lock().map_err(|_| {
            error(
                Code::Conflict,
                "此会话状态目录已被其他宿主占用，或文件系统不支持本地锁。",
            )
        })?;
        let store = Arc::new(Self {
            root,
            workspace,
            gate: Mutex::new(()),
            _lease: lease,
            #[cfg(feature = "compaction")]
            compactor: Mutex::new(None),
        });
        // Only a fresh owner of this authority namespace may declare old runs interrupted.
        let (headers, _) = store.headers()?;
        for header in headers.into_iter().filter(|h| h.status == Status::Running) {
            let mut doc = match store.load(&header.id) {
                Ok(doc) => doc,
                Err(_) => {
                    eprintln!("session {} is unreadable; left untouched", header.id);
                    continue;
                }
            };
            let end = match doc.history() {
                Ok(history) if !doc.body.turns.is_empty() => history.len(),
                _ => {
                    eprintln!("session {} has invalid history; left untouched", header.id);
                    continue;
                }
            };
            let turn = doc.body.turns.last_mut().ok_or_else(corrupt)?;
            turn.status = Status::Interrupted;
            turn.finished_at = Some(now_ms());
            turn.end = end;
            turn.error = Some(AgentError::new(ErrorCode::Closed,
                "上次进程已中断；已恢复确认过的记录，未结算的工具可能已产生外部影响，不会自动重跑。"));
            doc.header.status = Status::Interrupted;
            store.save(&mut doc, None)?;
        }
        Ok(store)
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join(format!("{id}.jsonl")))
    }
    fn read_header(&self, path: &Path) -> Result<Header> {
        regular_path(path, false)?;
        let mut bytes = Vec::new();
        BufReader::new(
            File::open(path)
                .map_err(io_error)?
                .take(MAX_HEADER_BYTES as u64 + 1),
        )
        .read_until(b'\n', &mut bytes)
        .map_err(io_error)?;
        if bytes.len() > MAX_HEADER_BYTES || bytes.last() != Some(&b'\n') {
            return Err(corrupt());
        }
        let header: Header = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
        validate_id(&header.id).map_err(|_| corrupt())?;
        if header.version != FORMAT
            || !Path::new(&header.workspace).is_absolute()
            || header.workspace.len() > 4096
            || header.metadata.len() > 16
            || header
                .metadata
                .iter()
                .any(|(k, v)| k.len() > 128 || v.len() > 4096)
            || header.title.len() > 320
            || path.file_stem().and_then(|v| v.to_str()) != Some(header.id.as_str())
        {
            return Err(corrupt());
        }
        Ok(header)
    }
    fn headers(&self) -> Result<(Vec<Header>, usize)> {
        let mut headers = Vec::new();
        let mut unreadable = 0;
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.path().extension().and_then(|v| v.to_str()) != Some("jsonl") {
                continue;
            }
            if headers.len() + unreadable >= MAX_SESSIONS {
                return Err(error(
                    Code::Capacity,
                    "会话目录达到扫描上限，请归档旧会话。",
                ));
            }
            match self.read_header(&entry.path()) {
                Ok(h) => headers.push(h),
                Err(_) => unreadable += 1,
            }
        }
        // Pinned conversations stay on top; everything else follows the newest update first.
        headers.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok((headers, unreadable))
    }
    fn load(&self, id: &str) -> Result<Document> {
        let path = self.path(id)?;
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(error(Code::NotFound, "会话不存在或已删除。"))
            }
            Err(e) => return Err(io_error(e)),
            _ => {}
        }
        let header = self.read_header(&path)?;
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(io_error)?
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_FILE_BYTES || bytes.last() != Some(&b'\n') {
            return Err(corrupt());
        }
        let split = bytes.iter().position(|b| *b == b'\n').ok_or_else(corrupt)?;
        let body: Body = serde_json::from_slice(&bytes[split + 1..]).map_err(|_| corrupt())?;
        if body.turns.len() != header.turn_count
            || body.turns.len() > MAX_TURNS
            || body.checkpoint.is_some() != body.checkpoint_turn.is_some()
            || body.checkpoint_turn.is_some_and(|n| n >= body.turns.len())
        {
            return Err(corrupt());
        }
        let mut keys = BTreeSet::new();
        for turn in &body.turns {
            validate_id(&turn.id).map_err(|_| corrupt())?;
            if !keys.insert(&turn.id) || turn.start > turn.end || turn.prompt.len() > 64 * 1024 {
                return Err(corrupt());
            }
        }
        if let (Some(cp), Some(index)) = (&body.checkpoint, body.checkpoint_turn) {
            if cp.schema_version != CHECKPOINT_VERSION
                || cp.metadata.get(SESSION_KEY) != Some(&header.id)
                || cp.metadata.get(TURN_KEY) != Some(&body.turns[index].id)
                || body.turns[index].run_id.as_deref() != Some(cp.run_id.as_str())
            {
                return Err(corrupt());
            }
        }
        let doc = Document { header, body };
        let mut end = 0;
        for (index, turn) in doc.body.turns.iter().enumerate() {
            if turn.start != end
                || turn.end <= turn.start
                || turn.prompt.trim().is_empty()
                || turn.status == Status::Idle
                || (turn.status == Status::Running) != turn.finished_at.is_none()
                || (turn.status == Status::Running && index + 1 != doc.body.turns.len())
            {
                return Err(corrupt());
            }
            end = turn.end;
        }
        if doc.header.status
            != doc
                .body
                .turns
                .last()
                .map_or(Status::Idle, |turn| turn.status)
            || end != doc.history()?.len()
        {
            return Err(corrupt());
        }
        Ok(doc)
    }
    fn save(&self, doc: &mut Document, cancel: Option<&CancellationToken>) -> Result<()> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(error(Code::Closed, "会话写入已取消。"));
        }
        doc.header.revision = doc.header.revision.checked_add(1).ok_or_else(corrupt)?;
        doc.header.updated_at = now_ms();
        doc.header.turn_count = doc.body.turns.len();
        let header = serde_json::to_vec(&doc.header).map_err(io_error)?;
        let body = serde_json::to_vec(&doc.body).map_err(io_error)?;
        if header.len() + 1 > MAX_HEADER_BYTES
            || header.len().saturating_add(body.len()).saturating_add(2) > MAX_FILE_BYTES
        {
            return Err(error(
                Code::Capacity,
                "会话文件达到 32 MiB 上限；未覆盖旧记录，请新建会话。",
            ));
        }
        let path = self.path(&doc.header.id)?;
        if path.exists() {
            regular_path(&path, false)?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(&self.root).map_err(io_error)?;
        temp.write_all(&header)
            .and_then(|_| temp.write_all(b"\n"))
            .and_then(|_| temp.write_all(&body))
            .and_then(|_| temp.write_all(b"\n"))
            .map_err(io_error)?;
        temp.as_file().sync_all().map_err(io_error)?;
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(error(Code::Closed, "会话写入已取消。"));
        }
        // Same-directory atomic replace: a reader sees one complete header + checkpoint generation.
        temp.persist(path)
            .map_err(io_error)?
            .sync_all()
            .map_err(io_error)?;
        self.sync_directory()
    }
    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(io_error)?;
        Ok(())
    }
    // `archived` selects the view: false lists the active tasks, true lists the archived ones.
    pub fn list(
        &self,
        offset: usize,
        limit: usize,
        query: &str,
        archived: bool,
    ) -> Result<Listing> {
        self.list_matching(offset, limit, query, archived, |_| true)
    }
    /// Lightweight identity lookup; does not load the transcript or require cwd to exist.
    pub fn header(&self, id: &str) -> Result<Header> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let path = self.path(id)?;
        if !path.exists() {
            return Err(error(Code::NotFound, "会话不存在或已删除。"));
        }
        self.read_header(&path)
    }
    /// Host-supplied, non-I/O navigation filter. Must not re-enter this Store.
    pub fn list_matching(
        &self,
        offset: usize,
        limit: usize,
        query: &str,
        archived: bool,
        filter: impl Fn(&Header) -> bool,
    ) -> Result<Listing> {
        let _guard = self.gate.lock().map_err(io_error)?;
        if !(1..=100).contains(&limit) || query.len() > 320 {
            return Err(error(Code::InvalidRequest, "分页或搜索参数超限。"));
        }
        let (mut headers, unreadable) = self.headers()?;
        let query = query.to_lowercase();
        headers.retain(|h| {
            h.archived == archived && h.title.to_lowercase().contains(&query) && filter(h)
        });
        let end = offset.saturating_add(limit);
        let next_offset = (end < headers.len()).then_some(end);
        Ok(Listing {
            sessions: headers.into_iter().skip(offset).take(limit).collect(),
            next_offset,
            unreadable,
        })
    }
    pub fn get(&self, id: &str) -> Result<Document> {
        let _guard = self.gate.lock().map_err(io_error)?;
        self.load(id)
    }
    /// Convenience for an explicit default directory supplied by the embedding host.
    pub fn create(&self, key: &str) -> Result<Header> {
        self.create_in(
            key,
            Path::new(&self.workspace),
            std::collections::BTreeMap::new(),
        )
    }
    /// Bind one session permanently to a trusted cwd. Projects are not a dependency.
    pub fn create_in(
        &self,
        key: &str,
        workspace: &Path,
        metadata: std::collections::BTreeMap<String, String>,
    ) -> Result<Header> {
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(error(
                Code::InvalidRequest,
                "会话工作目录必须是已存在的绝对目录。",
            ));
        }
        let workspace = fs::canonicalize(workspace).map_err(io_error)?;
        let workspace = workspace.to_str().ok_or_else(corrupt)?.to_owned();
        if workspace.len() > 4096
            || metadata.len() > 16
            || metadata
                .iter()
                .any(|(k, v)| k.len() > 128 || v.len() > 4096)
            || serde_json::to_vec(&metadata).map_err(io_error)?.len() > 8192
        {
            return Err(error(Code::InvalidRequest, "会话目录或元数据超限。"));
        }
        validate_id(key)?;
        let id = format!("s-{key}");
        validate_id(&id)?;
        let _guard = self.gate.lock().map_err(io_error)?;
        match self.load(&id) {
            Ok(doc) => {
                return if doc.header.workspace == workspace && doc.header.metadata == metadata {
                    Ok(doc.header)
                } else {
                    Err(error(
                        Code::Conflict,
                        "同一会话创建请求不能改变工作目录或归属。",
                    ))
                }
            }
            Err(e) if e.code == Code::NotFound => {}
            Err(e) => return Err(e),
        }
        let (existing, unreadable) = self.headers()?;
        if existing.len() + unreadable >= MAX_SESSIONS {
            return Err(error(Code::Capacity, "会话数量达到上限，请归档旧会话。"));
        }
        let mut doc = Document {
            header: Header {
                version: FORMAT,
                id,
                workspace,
                metadata,
                title: "新会话".into(),
                revision: 0,
                created_at: now_ms(),
                updated_at: now_ms(),
                turn_count: 0,
                status: Status::Idle,
                pinned: false,
                archived: false,
                unread: false,
            },
            body: Body {
                turns: Vec::new(),
                checkpoint: None,
                checkpoint_turn: None,
                archive: Vec::new(),
                base: None,
                compaction: None,
            },
        };
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
    fn editable(doc: &Document, revision: u64) -> Result<()> {
        if doc.header.revision != revision {
            return Err(error(Code::Conflict, "会话已更新，请刷新历史后再操作。"));
        }
        if doc.header.status == Status::Running {
            return Err(error(
                Code::Conflict,
                "该会话仍有任务运行，请等待结束或先停止。",
            ));
        }
        Ok(())
    }
    pub fn rename(&self, id: &str, revision: u64, title: &str) -> Result<Header> {
        let title = title.trim();
        if title.is_empty()
            || title.len() > 320
            || title.chars().count() > 80
            || title.chars().any(char::is_control)
        {
            return Err(error(
                Code::InvalidRequest,
                "会话标题需要 1–80 个字符，不能含控制字符。",
            ));
        }
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        Self::editable(&doc, revision)?;
        doc.header.title = title.into();
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
    pub fn delete(&self, id: &str, revision: u64) -> Result<()> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let doc = self.load(id)?;
        Self::editable(&doc, revision)?;
        fs::remove_file(self.path(id)?).map_err(io_error)?;
        self.sync_directory()
    }
    // Navigation flags change no execution state, so a running task may still be pinned or archived;
    // the revision alone guards against overwriting a newer header.
    fn set_flag(&self, id: &str, revision: u64, apply: impl FnOnce(&mut Header)) -> Result<Header> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        if doc.header.revision != revision {
            return Err(error(Code::Conflict, "会话已更新，请刷新列表后再操作。"));
        }
        apply(&mut doc.header);
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
    pub fn set_pinned(&self, id: &str, revision: u64, pinned: bool) -> Result<Header> {
        self.set_flag(id, revision, |header| header.pinned = pinned)
    }
    pub fn set_archived(&self, id: &str, revision: u64, archived: bool) -> Result<Header> {
        self.set_flag(id, revision, |header| header.archived = archived)
    }
    pub fn set_unread(&self, id: &str, revision: u64, unread: bool) -> Result<Header> {
        self.set_flag(id, revision, |header| header.unread = unread)
    }
    /// Admit one turn durably. The host supplies its actual history budget (including the new prompt).
    pub fn prepare(
        &self,
        id: &str,
        revision: u64,
        key: &str,
        prompt: &str,
        admission_bytes: usize,
    ) -> Result<Prepared> {
        self.prepare_checked(id, revision, key, prompt, admission_bytes, |_| Ok(()))
    }
    /// Like prepare, with a pure host check under the same revision/admission lock.
    /// The callback sees the exact next working history; failure leaves the file unchanged.
    /// It must not do I/O or re-enter this Store. Duplicate requests bypass the callback.
    pub fn prepare_checked(
        &self,
        id: &str,
        revision: u64,
        key: &str,
        prompt: &str,
        admission_bytes: usize,
        check: impl FnOnce(&[Message]) -> Result<()>,
    ) -> Result<Prepared> {
        validate_id(key)?;
        if prompt.trim().is_empty() || prompt.len() > 64 * 1024 {
            return Err(error(Code::InvalidRequest, "消息不能为空或超过 64 KiB。"));
        }
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        if let Some(turn) = doc.body.turns.iter().find(|t| t.id == key) {
            if turn.prompt != prompt {
                return Err(error(Code::Conflict, "同一请求 ID 不能提交不同内容。"));
            }
            return Ok(Prepared::Existing(turn.clone(), doc.header));
        }
        Self::editable(&doc, revision)?;
        if doc.body.turns.len() >= MAX_TURNS {
            return Err(error(Code::Capacity, "会话达到 512 轮上限，请新建会话。"));
        }
        let canonical_len = doc.history()?.len();
        let history = doc.begin_workset(self.compaction_enabled(), prompt, admission_bytes)?;
        check(&history)?;
        if doc.body.turns.is_empty() && doc.header.title == "新会话" {
            doc.header.title = prompt
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(80)
                .collect();
        }
        doc.body.turns.push(Turn {
            id: key.into(),
            prompt: prompt.into(),
            run_id: None,
            started_at: now_ms(),
            finished_at: None,
            status: Status::Running,
            start: canonical_len,
            end: canonical_len + 1,
            usage: TaskUsage {
                model_calls: 0,
                reported_tokens: 0,
                usage_complete: false,
            },
            error: None,
        });
        doc.header.status = Status::Running;
        self.save(&mut doc, None)?; // User input and durable dedup identity precede runtime dispatch.
        Ok(Prepared::New { history })
    }
    pub fn bind(&self, id: &str, key: &str, run_id: &str) -> Result<Header> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        let turn = doc
            .body
            .turns
            .iter_mut()
            .find(|t| t.id == key)
            .ok_or_else(corrupt)?;
        match &turn.run_id {
            Some(existing) if existing == run_id => return Ok(doc.header),
            Some(_) => return Err(corrupt()),
            None => turn.run_id = Some(run_id.into()),
        }
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
    pub fn fail_start(&self, id: &str, key: &str, message: &str) -> Result<()> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        let turn = doc
            .body
            .turns
            .last_mut()
            .filter(|t| t.id == key)
            .ok_or_else(corrupt)?;
        if turn.status != Status::Running {
            return Ok(());
        }
        turn.status = Status::Failed;
        turn.finished_at = Some(now_ms());
        turn.error = Some(AgentError::new(ErrorCode::Configuration, message));
        doc.header.status = Status::Failed;
        self.save(&mut doc, None)
    }
    pub fn commit(&self, cp: &RunCheckpoint, cancel: &CancellationToken) -> Result<()> {
        let (Some(id), Some(key)) = (cp.metadata.get(SESSION_KEY), cp.metadata.get(TURN_KEY))
        else {
            if cp.metadata.contains_key(SESSION_KEY) || cp.metadata.contains_key(TURN_KEY) {
                return Err(corrupt());
            }
            return Ok(()); // Unbound Runs remain explicitly ephemeral.
        };
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        let index = doc.body.turns.len().checked_sub(1).ok_or_else(corrupt)?;
        let turn = &doc.body.turns[index];
        if turn.id != *key || turn.run_id.as_ref().is_some_and(|run| run != &cp.run_id) {
            return Err(corrupt());
        }
        if let Some(old) = &doc.body.checkpoint {
            if old.run_id == cp.run_id && old.revision == cp.revision {
                return if serde_json::to_vec(old).map_err(io_error)?
                    == serde_json::to_vec(cp).map_err(io_error)?
                {
                    Ok(())
                } else {
                    Err(corrupt())
                };
            }
            if old.run_id == cp.run_id && cp.revision != old.revision + 1 {
                return Err(corrupt());
            }
        }
        if turn.status != Status::Running || cp.schema_version != CHECKPOINT_VERSION {
            return Err(corrupt());
        }
        doc.body.turns[index].run_id = Some(cp.run_id.clone());
        doc.body.checkpoint = Some(cp.clone());
        doc.body.checkpoint_turn = Some(index);
        if let Some(base) = &mut doc.body.base {
            base.pending_history = None;
        }
        if let Some(state) = self.compaction_state(&cp.run_id)? {
            #[cfg(feature = "compaction")]
            serde_json::from_value::<compaction::CompactionState>(state.clone())
                .map_err(|_| corrupt())?
                .project(&doc.working_history()?)
                .map_err(|_| corrupt())?;
            doc.body.compaction = Some(state);
        }
        let end = doc.history()?.len();
        let turn = &mut doc.body.turns[index];
        turn.end = end;
        turn.usage = cp.task_usage.clone();
        if cp.phase == CheckpointPhase::RunFinished {
            let status = cp.status.ok_or_else(corrupt)?;
            turn.status = Status::from_run(status);
            turn.finished_at = Some(now_ms());
            turn.error = cp.error.clone();
            doc.header.status = turn.status;
        }
        self.save(&mut doc, Some(cancel))
    }
    /// Failure-only settlement if a checkpoint failed. Keep the last acknowledged checkpoint;
    /// lost tool settlements remain unknown rather than being fabricated from UI events.
    pub fn finish(&self, id: &str, key: &str, report: &RunReport) -> Result<()> {
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        let saved_turn = doc
            .body
            .turns
            .iter()
            .find(|t| t.id == key)
            .ok_or_else(corrupt)?;
        if saved_turn.status != Status::Running {
            return Ok(());
        }
        let end = doc.history()?.len();
        let turn = doc
            .body
            .turns
            .last_mut()
            .filter(|t| t.id == key)
            .ok_or_else(corrupt)?;
        if report.status == RunStatus::Completed {
            return Err(corrupt());
        }
        turn.status = Status::from_run(report.status);
        turn.finished_at = Some(now_ms());
        turn.end = end;
        turn.error = report.error.clone();
        turn.usage = report.task_usage.clone();
        doc.header.status = turn.status;
        self.save(&mut doc, None)
    }
}

fn regular_path(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(io_error)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(corrupt());
        }
    }
    if meta.file_type().is_symlink()
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file()
        }
    {
        return Err(corrupt());
    }
    Ok(())
}
