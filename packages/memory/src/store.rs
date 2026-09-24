use api::CancellationToken;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAGIC: &str = "<!-- mona-memory:1 -->\n# Memory\n";
const MAX_FILE: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Conflict,
    Capacity,
    Forbidden,
    Closed,
    Io,
}
#[derive(Clone, Debug, Serialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(code: ErrorCode, message: &str) -> Error {
    Error {
        code,
        message: message.into(),
    }
}
fn io(_: impl std::fmt::Display) -> Error {
    error(
        ErrorCode::Io,
        "记忆读写失败；请检查目录权限、锁和磁盘空间，未将原文件当作空记忆。",
    )
}
fn invalid() -> Error {
    error(
        ErrorCode::InvalidRequest,
        "记忆格式无效或被外部不完整修改；请检查 MEMORY.md，原文件未覆盖。",
    )
}
fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(error(ErrorCode::Closed, "记忆操作已取消。"))
    } else {
        Ok(())
    }
}
// Waiting for another in-process operation must not outlive cancellation indefinitely.
// File I/O still relies on the host filesystem; a cancelled write is not a rollback.
fn acquire<'a, T>(mutex: &'a Mutex<T>, cancel: &CancellationToken) -> Result<MutexGuard<'a, T>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        check_cancel(cancel)?;
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(e)) => return Err(io(e)),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(error(ErrorCode::Conflict, "记忆操作等待超时，请稍后重新读取。"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_text_bytes: usize,
    pub max_entry_bytes: usize,
    pub max_entries: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_text_bytes: 8192,
            max_entry_bytes: 2048,
            max_entries: 64,
        }
    }
}
impl Limits {
    fn validate(self) -> Result<Self> {
        if self.max_text_bytes == 0
            || self.max_text_bytes > 16384
            || self.max_entry_bytes == 0
            || self.max_entry_bytes > self.max_text_bytes
            || self.max_entries == 0
            || self.max_entries > 128
        {
            return Err(error(ErrorCode::InvalidRequest, "记忆容量配置无效。"));
        }
        Ok(self)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub actor: String,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
}
impl Origin {
    pub fn host() -> Self {
        Self {
            actor: "user".into(),
            run_id: None,
            call_id: None,
        }
    }
    pub fn agent(run: &str, call: &str) -> Self {
        Self {
            actor: "agent".into(),
            run_id: Some(run.into()),
            call_id: Some(call.into()),
        }
    }
    fn validate(&self) -> Result<()> {
        let valid_actor = match self.actor.as_str() {
            "user" => self.run_id.is_none() && self.call_id.is_none(),
            "agent" => self.run_id.is_some() && self.call_id.is_some(),
            _ => false,
        };
        if !valid_actor
            || [&self.run_id, &self.call_id].iter().any(|v| {
                v.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 256 || s.chars().any(char::is_control)
                })
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub text: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub origin: Origin,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub revision: String,
    pub entries: Vec<Entry>,
    pub text_bytes: usize,
    pub limit_bytes: usize,
    pub markdown: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub revision: String,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Add { text: String },
    Replace { id: String, text: String },
    Remove { id: String },
}
/// Sync, bounded storage seam. Async hosts call it on a blocking worker.
pub trait Backend: Send + Sync {
    fn read(&self, cancel: &CancellationToken) -> Result<Snapshot>;
    fn apply(
        &self,
        change: &Change,
        origin: Origin,
        cancel: &CancellationToken,
    ) -> Result<Snapshot>;
}
#[derive(Clone, Default)]
struct Document {
    next_id: u64,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    next_id: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryMeta {
    id: String,
    created_at: u64,
    updated_at: u64,
    origin: Origin,
}
impl Document {
    fn render(&self) -> Result<String> {
        let mut s = format!(
            "{MAGIC}<!-- mona-state {} -->\n",
            serde_json::to_string(&State {
                next_id: self.next_id
            })
            .map_err(io)?
        );
        for e in &self.entries {
            let meta = EntryMeta {
                id: e.id.clone(),
                created_at: e.created_at,
                updated_at: e.updated_at,
                origin: e.origin.clone(),
            };
            s.push_str(&format!(
                "\n<!-- mona-entry {} -->\n{}\n",
                serde_json::to_string(&meta).map_err(io)?,
                e.text
            ));
        }
        if s.len() > MAX_FILE {
            return Err(error(
                ErrorCode::Capacity,
                "记忆文件含元数据超过 64 KiB；请缩减条目。",
            ));
        }
        Ok(s)
    }
    fn parse(text: &str, limits: Limits) -> Result<Self> {
        let rest = text.strip_prefix(MAGIC).ok_or_else(invalid)?;
        let (head, rest) = rest.split_once('\n').ok_or_else(invalid)?;
        let head = head
            .strip_prefix("<!-- mona-state ")
            .and_then(|s| s.strip_suffix(" -->"))
            .ok_or_else(invalid)?;
        let state: State = serde_json::from_str(head).map_err(|_| invalid())?;
        let mut doc = Self {
            next_id: state.next_id,
            entries: vec![],
        };
        let mut parts = rest.split("\n<!-- mona-entry ");
        if parts.next() != Some("") {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for part in parts {
            let (meta, text) = part.split_once(" -->\n").ok_or_else(invalid)?;
            let meta: EntryMeta = serde_json::from_str(meta).map_err(|_| invalid())?;
            let n = meta
                .id
                .strip_prefix("m-")
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(invalid)?;
            if n >= state.next_id
                || !ids.insert(meta.id.clone())
                || meta.updated_at < meta.created_at
            {
                return Err(invalid());
            }
            meta.origin.validate()?;
            let text = text.strip_suffix('\n').ok_or_else(invalid)?.to_owned();
            validate_text(&text, limits)?;
            doc.entries.push(Entry {
                id: meta.id,
                text,
                created_at: meta.created_at,
                updated_at: meta.updated_at,
                origin: meta.origin,
            });
        }
        doc.validate(limits)?;
        // Never flush a partial parse over human-edited or malformed content.
        if doc.render()? != text {
            return Err(invalid());
        }
        Ok(doc)
    }
    fn validate(&self, limits: Limits) -> Result<()> {
        if self.entries.len() > limits.max_entries
            || self.entries.iter().map(|e| e.text.len()).sum::<usize>() > limits.max_text_bytes
        {
            return Err(error(
                ErrorCode::Capacity,
                "长期记忆容量已满；请明确合并、缩短或删除过时条目，不会自动淘汰。",
            ));
        }
        Ok(())
    }
    fn snapshot(&self, limits: Limits) -> Result<Snapshot> {
        let markdown = self.render()?;
        Ok(Snapshot {
            revision: digest(&markdown),
            text_bytes: self.entries.iter().map(|e| e.text.len()).sum(),
            limit_bytes: limits.max_text_bytes,
            entries: self.entries.clone(),
            markdown,
        })
    }
    fn changed(&self, change: &Change, origin: Origin, limits: Limits) -> Result<Self> {
        if change.revision != digest(&self.render()?) {
            return Err(error(
                ErrorCode::Conflict,
                "记忆已更新；请先重新读取，再针对新的 revision 修改。",
            ));
        }
        if change.operations.is_empty() || change.operations.len() > 16 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "每批记忆变更需要 1–16 个操作。",
            ));
        }
        origin.validate()?;
        let mut next = self.clone();
        let at = now();
        for op in &change.operations {
            match op {
                Operation::Add { text } => {
                    validate_text(text, limits)?;
                    if next.entries.iter().any(|e| e.text == *text) {
                        continue;
                    }
                    let id = format!("m-{}", next.next_id);
                    next.next_id = next.next_id.checked_add(1).ok_or_else(invalid)?;
                    next.entries.push(Entry {
                        id,
                        text: text.clone(),
                        created_at: at,
                        updated_at: at,
                        origin: origin.clone(),
                    });
                }
                Operation::Replace { id, text } => {
                    validate_text(text, limits)?;
                    let e = next
                        .entries
                        .iter_mut()
                        .find(|e| &e.id == id)
                        .ok_or_else(|| {
                            error(ErrorCode::Conflict, "记忆条目已不存在；请重新读取。")
                        })?;
                    if e.text != *text {
                        e.text = text.clone();
                        e.updated_at = at.max(e.created_at);
                        e.origin = origin.clone();
                    }
                }
                Operation::Remove { id } => {
                    let pos = next
                        .entries
                        .iter()
                        .position(|e| &e.id == id)
                        .ok_or_else(|| {
                            error(ErrorCode::Conflict, "记忆条目已不存在；请重新读取。")
                        })?;
                    next.entries.remove(pos);
                }
            }
        }
        next.validate(limits)?;
        next.render()?;
        Ok(next)
    }
}
fn validate_text(text: &str, limits: Limits) -> Result<()> {
    if text.trim().is_empty()
        || text.trim() != text
        || text.len() > limits.max_entry_bytes
        || text.contains("<!-- mona-")
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(error(
            ErrorCode::InvalidRequest,
            "记忆正文应为无首尾空白的完整条目，不含保留标记/控制字符，单条不得超过配置上限。",
        ));
    }
    if text.contains("PRIVATE KEY-----") || text.contains("-----BEGIN OPENSSH PRIVATE KEY") {
        return Err(error(ErrorCode::Forbidden, "不能将私钥保存为长期记忆。"));
    }
    Ok(())
}
fn regular(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(io)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(error(ErrorCode::Forbidden, "记忆文件和目录不能是链接。"));
        }
    }
    if m.file_type().is_symlink() || if directory { !m.is_dir() } else { !m.is_file() } {
        return Err(error(
            ErrorCode::Forbidden,
            "记忆文件和目录不能是链接或特殊文件。",
        ));
    }
    Ok(())
}
pub struct FileStore {
    root: PathBuf,
    limits: Limits,
    gate: Mutex<()>,
}
impl FileStore {
    pub fn open(root: &Path, limits: Limits) -> Result<Self> {
        let limits = limits.validate()?;
        fs::create_dir_all(root).map_err(io)?;
        regular(root, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(io)?;
        }
        let store = Self {
            root: root.into(),
            limits,
            gate: Mutex::new(()),
        };
        store.read(&CancellationToken::new())?;
        Ok(store)
    }
    fn locked(&self) -> Result<File> {
        regular(&self.root, true)?;
        let path = self.root.join("memory.lock");
        match fs::symlink_metadata(&path) {
            Ok(_) => regular(&path, false)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(io)?;
        file.try_lock()
            .map_err(|_| error(ErrorCode::Conflict, "记忆正在由其他操作修改，请稍后重试。"))?;
        Ok(file)
    }
    fn load(&self) -> Result<Document> {
        let path = self.root.join("MEMORY.md");
        match fs::symlink_metadata(&path) {
            Ok(_) => regular(&path, false)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Document::default()),
            Err(e) => return Err(io(e)),
        }
        let mut text = String::new();
        File::open(path)
            .map_err(io)?
            .take(MAX_FILE as u64 + 1)
            .read_to_string(&mut text)
            .map_err(io)?;
        if text.len() > MAX_FILE {
            return Err(error(ErrorCode::Capacity, "记忆文件超过读取上限，未覆盖。"));
        }
        Document::parse(&text, self.limits)
    }
}
impl Backend for FileStore {
    fn read(&self, cancel: &CancellationToken) -> Result<Snapshot> {
        check_cancel(cancel)?;
        let _g = acquire(&self.gate, cancel)?;
        let _file = self.locked()?;
        check_cancel(cancel)?;
        let snapshot = self.load()?.snapshot(self.limits)?;
        check_cancel(cancel)?;
        Ok(snapshot)
    }
    fn apply(
        &self,
        change: &Change,
        origin: Origin,
        cancel: &CancellationToken,
    ) -> Result<Snapshot> {
        check_cancel(cancel)?;
        let _g = acquire(&self.gate, cancel)?;
        let _file = self.locked()?;
        check_cancel(cancel)?;
        let current = self.load()?;
        let next = current.changed(change, origin, self.limits)?;
        let snapshot = next.snapshot(self.limits)?;
        if snapshot.revision == change.revision {
            return Ok(snapshot);
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root).map_err(io)?;
        tmp.write_all(snapshot.markdown.as_bytes()).map_err(io)?;
        tmp.as_file().sync_all().map_err(io)?;
        check_cancel(cancel)?;
        tmp.persist(self.root.join("MEMORY.md"))
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        #[cfg(unix)]
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        Ok(snapshot)
    }
}
/// Explicit ephemeral backend; never a fallback for failed persistent storage.
pub struct InMemoryStore {
    state: Mutex<Document>,
    limits: Limits,
}
impl Default for InMemoryStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(Document::default()),
            limits: Limits::default(),
        }
    }
}
impl InMemoryStore {
    pub fn new(limits: Limits) -> Result<Self> {
        Ok(Self {
            state: Mutex::new(Document::default()),
            limits: limits.validate()?,
        })
    }
}
impl Backend for InMemoryStore {
    fn read(&self, cancel: &CancellationToken) -> Result<Snapshot> {
        check_cancel(cancel)?;
        let state = acquire(&self.state, cancel)?;
        let snapshot = state.snapshot(self.limits)?;
        check_cancel(cancel)?;
        Ok(snapshot)
    }
    fn apply(
        &self,
        change: &Change,
        origin: Origin,
        cancel: &CancellationToken,
    ) -> Result<Snapshot> {
        check_cancel(cancel)?;
        let mut state = acquire(&self.state, cancel)?;
        let next = state.changed(change, origin, self.limits)?;
        let view = next.snapshot(self.limits)?;
        check_cancel(cancel)?;
        *state = next;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn cancelling_a_lock_wait_does_not_wait_for_the_lock_owner() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileStore::open(dir.path(), Limits::default()).unwrap());
        let held = store.gate.lock().unwrap();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let worker_store = store.clone();
        let (started, ready) = mpsc::channel();
        let (result, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            result.send(worker_store.read(&token)).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        let outcome = receive.recv_timeout(Duration::from_secs(1));
        drop(held);
        worker.join().unwrap();
        assert_eq!(outcome.expect("cancel must unblock the waiting operation").unwrap_err().code, ErrorCode::Closed);
        assert!(!dir.path().join("MEMORY.md").exists());
    }

    #[test]
    fn incomplete_provenance_is_rejected_before_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path(), Limits::default()).unwrap();
        let token = CancellationToken::new();
        let before = store.read(&token).unwrap();
        let change = Change { revision: before.revision.clone(), operations: vec![Operation::Add { text: "fact".into() }] };
        for origin in [
            Origin { actor: "agent".into(), run_id: None, call_id: None },
            Origin { actor: "user".into(), run_id: Some("untrusted".into()), call_id: Some("untrusted".into()) },
        ] {
            assert_eq!(store.apply(&change, origin, &token).unwrap_err().code, ErrorCode::InvalidRequest);
            assert_eq!(store.read(&token).unwrap().revision, before.revision);
        }
        assert!(!dir.path().join("MEMORY.md").exists());
    }
}
