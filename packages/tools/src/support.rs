use api::{AgentError, ErrorCode, Result, ToolContext};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

pub(crate) fn error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

pub(crate) fn resolve_path(cwd: &Path, value: &str) -> Result<PathBuf> {
    if value.is_empty() || value.contains('\0') {
        return Err(error(ErrorCode::Schema, "path must be nonempty text"));
    }
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

pub(crate) struct Mutation {
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
}

/// The blocking worker owns the path lock until the write settles, even if the
/// Runtime drops the awaiting future after its cancellation grace expires.
pub(crate) async fn mutate_file(
    path: PathBuf,
    ctx: &ToolContext,
    _config: &crate::ToolConfig,
    transform: impl FnOnce(Option<Vec<u8>>) -> Result<Vec<u8>> + Send + 'static,
) -> Result<Mutation> {
    let run = ctx.run.clone();
    #[cfg(feature = "sandbox")]
    let policy = _config.sandbox.as_ref().map(|binding| binding.resolve(&run)).transpose()?;
    tokio::task::spawn_blocking(move || {
        let check = || {
            run.task.check()?;
            if run.cancel.is_cancelled() {
                return Err(error(ErrorCode::Cancelled, "file mutation cancelled"));
            }
            Ok(())
        };
        check()?;
        #[cfg(feature = "sandbox")]
        let path = if let Some(policy) = &policy { policy.check_write(&path).map_err(crate::confinement::failure)? } else { canonical_destination(&path)? };
        #[cfg(not(feature = "sandbox"))]
        let path = canonical_destination(&path)?;
        let recheck = || -> Result<()> {
            check()?;
            #[cfg(feature = "sandbox")]
            if let Some(policy) = &policy {
                if policy.check_write(&path).map_err(crate::confinement::failure)? != path {
                    return Err(error(ErrorCode::Policy, "FS_SANDBOX_DENIED: destination changed before file mutation"));
                }
            }
            Ok(())
        };
        let lock = mutation_lock(&path);
        let _guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recheck()?;
        const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
        let (before, permissions) = match std::fs::File::open(&path) {
            Ok(file) => {
                let metadata = file.metadata().map_err(io_error)?;
                if !metadata.is_file() {
                    return Err(error(ErrorCode::Tool, "destination is not a regular file"));
                }
                if metadata.permissions().readonly() {
                    return Err(error(ErrorCode::Tool, "destination is read-only"));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(error(
                        ErrorCode::Limit,
                        "file mutation exceeds 32 MiB limit",
                    ));
                }
                let mut bytes = Vec::new();
                file.take(MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io_error)?;
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    return Err(error(
                        ErrorCode::Limit,
                        "file mutation exceeds 32 MiB limit",
                    ));
                }
                (Some(bytes), Some(metadata.permissions()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(e) => return Err(io_error(e)),
        };
        check()?;
        let after = transform(before.clone())?;
        if after.len() as u64 > MAX_FILE_BYTES {
            return Err(error(
                ErrorCode::Limit,
                "file mutation exceeds 32 MiB limit",
            ));
        }
        recheck()?;
        let parent = path
            .parent()
            .ok_or_else(|| error(ErrorCode::Tool, "destination has no parent directory"))?;
        std::fs::create_dir_all(parent).map_err(io_error)?;
        recheck()?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
        temporary.write_all(&after).map_err(io_error)?;
        if let Some(permissions) = permissions {
            temporary
                .as_file()
                .set_permissions(permissions)
                .map_err(io_error)?;
        }
        temporary.as_file().sync_all().map_err(io_error)?;
        recheck()?;
        temporary.persist(&path).map_err(|e| io_error(e.error))?;
        Ok(Mutation { before, after })
    })
    .await
    .map_err(|_| error(ErrorCode::Tool, "file mutation worker failed"))?
}

fn io_error(e: std::io::Error) -> AgentError {
    error(ErrorCode::Tool, format!("file operation failed: {e}"))
}

fn canonical_destination(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    let mut ancestor = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut found) => {
                for name in missing.into_iter().rev() {
                    found.push(name);
                }
                return Ok(found);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| error(ErrorCode::Tool, "invalid destination path"))?
                        .to_owned(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| error(ErrorCode::Tool, "invalid destination path"))?;
            }
            Err(e) => return Err(io_error(e)),
        }
    }
}

fn mutation_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    #[cfg(windows)]
    let key = PathBuf::from(path.to_string_lossy().to_lowercase());
    #[cfg(not(windows))]
    let key = path.to_owned();
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub(crate) fn clip_head_tail(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let marker = "\n[output truncated]\n";
    let available = max_bytes.saturating_sub(marker.len());
    let head = available / 2;
    let tail = available - head;
    let prefix = api::clip_utf8(text, head);
    let mut start = text.len().saturating_sub(tail);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (format!("{prefix}{marker}{}", &text[start..]), true)
}
