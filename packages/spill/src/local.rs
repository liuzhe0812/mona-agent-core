use crate::{
    error, validate_run_id, validate_spill_id, CleanupReport, SpillConfig, SpillPage, SpillRecord,
    SpillStore,
};
use async_trait::async_trait;
use std::{
    collections::BTreeSet,
    fs::Metadata,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(windows)]
use std::{sync::atomic::AtomicBool, time::Duration};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};

/// Local private-directory backend. Every run gets a hex-encoded directory,
/// while archive IDs remain opaque to callers.
#[derive(Clone)]
pub struct LocalSpillStore {
    root: Arc<PathBuf>,
    config: SpillConfig,
    lock: Arc<Mutex<()>>,
    sequence: Arc<AtomicU64>,
    #[cfg(windows)]
    root_permissions_ready: Arc<AtomicBool>,
}

impl LocalSpillStore {
    pub fn new(root: impl Into<PathBuf>, config: SpillConfig) -> crate::Result<Self> {
        config.validate()?;
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(error(
                api::ErrorCode::Configuration,
                "spill root must not be empty",
            ));
        }
        Ok(Self {
            root: Arc::new(root),
            config,
            lock: Arc::new(Mutex::new(())),
            sequence: Arc::new(AtomicU64::new(1)),
            #[cfg(windows)]
            root_permissions_ready: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn config(&self) -> &SpillConfig {
        &self.config
    }

    async fn ensure_root(&self) -> crate::Result<()> {
        let created = match fs::symlink_metadata(self.root.as_ref()).await {
            Ok(metadata) => {
                validate_directory_metadata(&metadata, "spill storage root")?;
                false
            }
            Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(self.root.as_ref())
                    .await
                    .map_err(|_| error(api::ErrorCode::Tool, "spill storage is not writable"))?;
                let metadata = fs::symlink_metadata(self.root.as_ref())
                    .await
                    .map_err(|_| {
                        error(
                            api::ErrorCode::Tool,
                            "spill storage root cannot be inspected",
                        )
                    })?;
                validate_directory_metadata(&metadata, "spill storage root")?;
                true
            }
            Err(_) => {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill storage root cannot be inspected",
                ))
            }
        };

        self.ensure_root_permissions(created).await?;
        Ok(())
    }

    async fn ensure_existing_root(&self) -> crate::Result<bool> {
        let metadata = match fs::symlink_metadata(self.root.as_ref()).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill storage root cannot be inspected",
                ))
            }
        };
        validate_directory_metadata(&metadata, "spill storage root")?;
        self.ensure_root_permissions(false).await?;
        Ok(true)
    }

    async fn ensure_root_permissions(&self, created: bool) -> crate::Result<()> {
        #[cfg(unix)]
        {
            let _ = created;
            set_private_directory_permissions(self.root.as_ref()).await?;
        }
        #[cfg(windows)]
        if created || !self.root_permissions_ready.load(Ordering::Acquire) {
            set_private_permissions(self.root.as_ref()).await?;
            self.root_permissions_ready.store(true, Ordering::Release);
        }
        Ok(())
    }

    async fn ensure_run_dir(&self, run_id: &str) -> crate::Result<PathBuf> {
        let path = self.run_dir(run_id);
        match fs::symlink_metadata(&path).await {
            Ok(metadata) => validate_directory_metadata(&metadata, "spill run directory")?,
            Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).await.map_err(|_| {
                    error(api::ErrorCode::Tool, "spill run storage is not writable")
                })?;
                let metadata = fs::symlink_metadata(&path).await.map_err(|_| {
                    error(
                        api::ErrorCode::Tool,
                        "spill run directory cannot be inspected",
                    )
                })?;
                validate_directory_metadata(&metadata, "spill run directory")?;
            }
            Err(_) => {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill run directory cannot be inspected",
                ))
            }
        }
        #[cfg(unix)]
        set_private_directory_permissions(&path).await?;
        Ok(path)
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(hex_component(run_id))
    }

    fn entry_path(&self, run_id: &str, id: &str) -> PathBuf {
        self.run_dir(run_id).join(format!("{id}.txt"))
    }

    async fn scan(&self) -> crate::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        match fs::symlink_metadata(self.root.as_ref()).await {
            Ok(metadata) => validate_directory_metadata(&metadata, "spill storage root")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(_) => {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill storage root cannot be inspected",
                ))
            }
        }
        let mut dirs = match fs::read_dir(self.root.as_ref()).await {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(_) => return Err(error(api::ErrorCode::Tool, "spill storage cannot be read")),
        };
        while let Some(dir) = dirs
            .next_entry()
            .await
            .map_err(|_| error(api::ErrorCode::Tool, "spill storage cannot be read"))?
        {
            let metadata = fs::symlink_metadata(dir.path())
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill storage cannot be inspected"))?;
            if is_link_or_reparse(&metadata) {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill run directory must not be a link",
                ));
            }
            if !metadata.is_dir() {
                continue;
            }
            let run_name = dir.file_name().to_string_lossy().into_owned();
            let mut files = fs::read_dir(dir.path())
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill run cannot be read"))?;
            while let Some(file) = files
                .next_entry()
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill run cannot be read"))?
            {
                let metadata = fs::symlink_metadata(file.path())
                    .await
                    .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be inspected"))?;
                if is_link_or_reparse(&metadata) {
                    return Err(error(
                        api::ErrorCode::Tool,
                        "spill entry must not be a link",
                    ));
                }
                if !metadata.is_file() {
                    continue;
                }
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.starts_with("sp_") || !name.ends_with(".txt") {
                    continue;
                }
                entries.push(Entry {
                    run_name: run_name.clone(),
                    path: file.path(),
                    bytes: metadata.len(),
                    modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                });
            }
        }
        Ok(entries)
    }

    async fn usage(&self) -> crate::Result<(usize, std::collections::BTreeMap<String, usize>)> {
        let entries = self.scan().await?;
        let total = entries.iter().map(|entry| entry.bytes as usize).sum();
        let mut per_run = std::collections::BTreeMap::new();
        for entry in entries {
            *per_run.entry(entry.run_name).or_insert(0usize) = per_run
                .get(&entry.run_name)
                .copied()
                .unwrap_or(0)
                .saturating_add(entry.bytes as usize);
        }
        Ok((total, per_run))
    }
}

/// Copy with a fixed buffer, validating UTF-8 across chunk boundaries. The declared
/// size reserves quota before I/O; both short and longer-than-declared streams fail.
async fn copy_utf8(source: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    target: &mut fs::File, expected: usize) -> crate::Result<()> {
    let mut source = source.take(expected.saturating_add(1) as u64);
    let mut buffer = [0u8; 16 * 1024];
    let mut tail = Vec::new();
    let mut total = 0usize;
    loop {
        let count = source.read(&mut buffer).await
            .map_err(|_| error(api::ErrorCode::Tool, "spill source cannot be read"))?;
        if count == 0 { break; }
        total = total.saturating_add(count);
        if total > expected { return Err(error(api::ErrorCode::Tool, "spill source size changed")); }
        let mut joined = std::mem::take(&mut tail);
        joined.extend_from_slice(&buffer[..count]);
        match std::str::from_utf8(&joined) {
            Ok(_) => {},
            Err(e) if e.error_len().is_none() => tail.extend_from_slice(&joined[e.valid_up_to()..]),
            Err(_) => return Err(error(api::ErrorCode::Tool, "spill source is not valid UTF-8")),
        }
        target.write_all(&buffer[..count]).await
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be written"))?;
    }
    if total != expected || !tail.is_empty() {
        return Err(error(api::ErrorCode::Tool, "spill source is incomplete or its size changed"));
    }
    Ok(())
}

#[derive(Clone)]
struct Entry {
    run_name: String,
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

#[async_trait]
impl SpillStore for LocalSpillStore {
    async fn put_stream(&self, run_id: &str, _call_id: &str,
        source: &mut (dyn tokio::io::AsyncRead + Unpin + Send), bytes: usize) -> crate::Result<SpillRecord> {
        validate_run_id(run_id)?;
        if bytes > self.config.max_entry_bytes {
            return Err(error(
                api::ErrorCode::Limit,
                "spill entry exceeds its byte limit",
            ));
        }
        let _guard = self.lock.lock().await;
        self.ensure_root().await?;
        let (total, per_run) = self.usage().await?;
        let run_name = hex_component(run_id);
        if per_run
            .get(&run_name)
            .copied()
            .unwrap_or(0)
            .saturating_add(bytes)
            > self.config.max_run_bytes
        {
            return Err(error(api::ErrorCode::Limit, "spill run quota exceeded"));
        }
        if total.saturating_add(bytes) > self.config.max_total_bytes {
            return Err(error(api::ErrorCode::Limit, "spill storage quota exceeded"));
        }
        let run_dir = self.ensure_run_dir(run_id).await?;
        let id = self.next_id();
        let final_path = run_dir.join(format!("{id}.txt"));
        reject_existing_entry(&final_path).await?;
        // RAII removes unfinished writes on error or future cancellation. The handle is
        // created inside the private archive directory; no temporary locator is exposed.
        let temporary = tempfile::Builder::new().prefix(".sp_").suffix(".tmp")
            .tempfile_in(&run_dir)
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be created"))?;
        #[cfg(unix)]
        set_private_file_permissions(temporary.path()).await?;
        let handle = temporary.as_file().try_clone()
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be opened"))?;
        let mut file = fs::File::from_std(handle);
        copy_utf8(source, &mut file, bytes).await?;
        file.flush().await.map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be written"))?;
        file.sync_all().await.map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be committed"))?;
        drop(file);
        validate_directory_metadata(&fs::symlink_metadata(&run_dir).await
            .map_err(|_| error(api::ErrorCode::Tool, "spill run directory cannot be inspected"))?, "spill run directory")?;
        // Non-overwriting publication is synchronous so cancellation cannot release
        // the quota lock while a detached publication task is still running.
        temporary.persist_noclobber(&final_path)
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be committed"))?;
        #[cfg(unix)]
        std::fs::File::open(&run_dir).and_then(|dir| dir.sync_all())
            .map_err(|_| error(api::ErrorCode::Tool, "spill directory cannot be committed"))?;
        Ok(SpillRecord { id, bytes: bytes as u64 })
    }

    async fn read_page(
        &self,
        run_id: &str,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> crate::Result<SpillPage> {
        validate_run_id(run_id)?;
        validate_spill_id(id)?;
        if limit == 0 || limit > self.config.max_page_bytes {
            return Err(error(
                api::ErrorCode::Limit,
                "spill page exceeds its byte limit",
            ));
        }
        if offset > self.config.max_entry_bytes {
            return Err(error(
                api::ErrorCode::Limit,
                "spill offset exceeds its byte limit",
            ));
        }
        let _guard = self.lock.lock().await;
        if !self.ensure_existing_root().await? {
            return Err(error(
                api::ErrorCode::Tool,
                "spill entry is unknown or expired",
            ));
        }
        let run_dir = self.run_dir(run_id);
        let run_metadata = fs::symlink_metadata(&run_dir).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::error(api::ErrorCode::Tool, "spill entry is unknown or expired")
            } else {
                crate::error(
                    api::ErrorCode::Tool,
                    "spill run directory cannot be inspected",
                )
            }
        })?;
        validate_directory_metadata(&run_metadata, "spill run directory")?;
        let path = self.entry_path(run_id, id);
        let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::error(api::ErrorCode::Tool, "spill entry is unknown or expired")
            } else {
                crate::error(api::ErrorCode::Tool, "spill entry cannot be inspected")
            }
        })?;
        validate_file_metadata(&metadata, "spill entry")?;
        let total = usize::try_from(metadata.len())
            .map_err(|_| error(api::ErrorCode::Limit, "spill entry exceeds its byte limit"))?;
        if offset > total {
            return Err(error(
                api::ErrorCode::Schema,
                "spill offset is past the end of the entry",
            ));
        }
        let mut read_options = fs::OpenOptions::new();
        read_options.read(true);
        #[cfg(windows)]
        {
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            read_options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = read_options
            .open(&path)
            .await
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be read"))?;
        if offset == total {
            return Ok(SpillPage {
                id: id.to_owned(),
                offset,
                next_offset: offset,
                total_bytes: total,
                eof: true,
                text: String::new(),
            });
        }
        file.seek(SeekFrom::Start(offset as u64))
            .await
            .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be read"))?;
        let read_len = limit.min(total - offset);
        let mut bytes = vec![0u8; read_len];
        let mut filled = 0;
        let mut reached_eof = false;
        while filled < read_len {
            let count = file
                .read(&mut bytes[filled..])
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be read"))?;
            if count == 0 {
                reached_eof = true;
                break;
            }
            filled += count;
        }
        if filled == 0 || (reached_eof && filled < read_len) {
            return Err(error(
                api::ErrorCode::Tool,
                "spill entry changed while being read",
            ));
        }
        bytes.truncate(filled);
        if is_utf8_continuation(bytes[0]) {
            return Err(error(
                api::ErrorCode::Schema,
                "spill offset is not a UTF-8 character boundary",
            ));
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(utf8_error) if utf8_error.error_len().is_none() && offset + filled < total => {
                let valid = utf8_error.valid_up_to();
                if valid == 0 {
                    return Err(error(
                        api::ErrorCode::Limit,
                        "spill page limit is smaller than one UTF-8 character",
                    ));
                }
                std::str::from_utf8(&bytes[..valid])
                    .map_err(|_| error(api::ErrorCode::Tool, "spill entry is not valid UTF-8"))?
            }
            Err(_) => {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill entry is not valid UTF-8",
                ))
            }
        };
        let next_offset = offset + text.len();
        Ok(SpillPage {
            id: id.to_owned(),
            offset,
            next_offset,
            total_bytes: total,
            eof: next_offset >= total,
            text: text.to_owned(),
        })
    }

    async fn cleanup(&self, active_runs: &BTreeSet<String>) -> crate::Result<CleanupReport> {
        for run_id in active_runs {
            validate_run_id(run_id)?;
        }
        let _guard = self.lock.lock().await;
        if !self.ensure_existing_root().await? {
            return Ok(CleanupReport::default());
        }
        let entries = self.scan().await?;
        let now = SystemTime::now();
        let mut report = CleanupReport::default();
        let active_names: BTreeSet<String> =
            active_runs.iter().map(|run| hex_component(run)).collect();
        for entry in entries {
            if active_names.contains(&entry.run_name)
                || now.duration_since(entry.modified).unwrap_or_default() < self.config.retention
            {
                continue;
            }
            match fs::remove_file(&entry.path).await {
                Ok(()) => {
                    report.removed_entries += 1;
                    report.removed_bytes =
                        report.removed_bytes.saturating_add(entry.bytes as usize);
                }
                Err(_) => return Err(error(api::ErrorCode::Tool, "spill entry cannot be removed")),
            }
        }
        // Remove stale temporary files only after the same retention window;
        // an active run's in-progress write is left alone.
        let mut dirs = match fs::read_dir(self.root.as_ref()).await {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(_) => return Err(error(api::ErrorCode::Tool, "spill storage cannot be read")),
        };
        while let Some(dir) = dirs
            .next_entry()
            .await
            .map_err(|_| error(api::ErrorCode::Tool, "spill storage cannot be read"))?
        {
            let metadata = fs::symlink_metadata(dir.path())
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill storage cannot be inspected"))?;
            if is_link_or_reparse(&metadata) {
                return Err(error(
                    api::ErrorCode::Tool,
                    "spill run directory must not be a link",
                ));
            }
            if !metadata.is_dir() {
                continue;
            }
            let run_name = dir.file_name().to_string_lossy().into_owned();
            if active_names.contains(&run_name) {
                continue;
            }
            let mut files = fs::read_dir(dir.path())
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill run cannot be read"))?;
            while let Some(file) = files
                .next_entry()
                .await
                .map_err(|_| error(api::ErrorCode::Tool, "spill run cannot be read"))?
            {
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.starts_with(".sp_") || !name.ends_with(".tmp") {
                    continue;
                }
                let metadata = fs::symlink_metadata(file.path())
                    .await
                    .map_err(|_| error(api::ErrorCode::Tool, "spill entry cannot be inspected"))?;
                if is_link_or_reparse(&metadata) {
                    return Err(error(
                        api::ErrorCode::Tool,
                        "spill entry must not be a link",
                    ));
                }
                if now
                    .duration_since(metadata.modified().unwrap_or(UNIX_EPOCH))
                    .unwrap_or_default()
                    >= self.config.retention
                {
                    fs::remove_file(file.path()).await.map_err(|_| {
                        error(
                            api::ErrorCode::Tool,
                            "spill temporary entry cannot be removed",
                        )
                    })?;
                }
            }
            match fs::remove_dir(dir.path()).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(_) => {
                    return Err(error(
                        api::ErrorCode::Tool,
                        "spill run directory cannot be removed",
                    ))
                }
            }
        }
        Ok(report)
    }
}

impl LocalSpillStore {
    fn next_id(&self) -> String {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        format!("sp_{:016x}{:08x}", now.as_nanos(), sequence)
    }
}

fn hex_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        output.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        output.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    output
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_directory_metadata(metadata: &Metadata, label: &str) -> crate::Result<()> {
    if is_link_or_reparse(metadata) {
        return Err(error(
            api::ErrorCode::Tool,
            format!("{label} must not be a link"),
        ));
    }
    if !metadata.is_dir() {
        return Err(error(
            api::ErrorCode::Tool,
            format!("{label} must be a directory"),
        ));
    }
    Ok(())
}

fn validate_file_metadata(metadata: &Metadata, label: &str) -> crate::Result<()> {
    if is_link_or_reparse(metadata) {
        return Err(error(
            api::ErrorCode::Tool,
            format!("{label} must not be a link"),
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            api::ErrorCode::Tool,
            format!("{label} must be a regular file"),
        ));
    }
    Ok(())
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

async fn reject_existing_entry(path: &Path) -> crate::Result<()> {
    match fs::symlink_metadata(path).await {
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(error(api::ErrorCode::Tool, "spill entry already exists")),
        Err(_) => Err(error(
            api::ErrorCode::Tool,
            "spill entry cannot be inspected",
        )),
    }
}

#[cfg(unix)]
async fn set_private_directory_permissions(path: &Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path).await.map_err(|_| {
        error(
            api::ErrorCode::Tool,
            "spill directory permissions cannot be inspected",
        )
    })?;
    validate_directory_metadata(&metadata, "spill directory")?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).await.map_err(|_| {
        error(
            api::ErrorCode::Tool,
            "spill directory permissions cannot be set",
        )
    })?;
    let mode = fs::symlink_metadata(path)
        .await
        .map_err(|_| {
            error(
                api::ErrorCode::Tool,
                "spill directory permissions cannot be verified",
            )
        })?
        .permissions()
        .mode();
    if mode & 0o777 != 0o700 {
        return Err(error(
            api::ErrorCode::Tool,
            "spill directory permissions are not private",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn set_private_file_permissions(path: &Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path).await.map_err(|_| {
        error(
            api::ErrorCode::Tool,
            "spill entry permissions cannot be inspected",
        )
    })?;
    validate_file_metadata(&metadata, "spill entry")?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).await.map_err(|_| {
        error(
            api::ErrorCode::Tool,
            "spill entry permissions cannot be set",
        )
    })?;
    let mode = fs::symlink_metadata(path)
        .await
        .map_err(|_| {
            error(
                api::ErrorCode::Tool,
                "spill entry permissions cannot be verified",
            )
        })?
        .permissions()
        .mode();
    if mode & 0o777 != 0o600 {
        return Err(error(
            api::ErrorCode::Tool,
            "spill entry permissions are not private",
        ));
    }
    Ok(())
}

#[cfg(windows)]
async fn set_private_permissions(path: &Path) -> crate::Result<()> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1') -Force -ErrorAction Stop
$path = $env:MONA_SPILL_ACL_PATH
if ([string]::IsNullOrWhiteSpace($path)) { throw 'missing spill ACL path' }
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$rights = [System.Security.AccessControl.FileSystemRights]::FullControl
$inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
function Set-PrivateAcl([string]$itemPath, [bool]$isDirectory) {
    $existing = Get-Acl -LiteralPath $itemPath
    $private = $existing.AreAccessRulesProtected
    $found = $false
    foreach ($entry in $existing.Access) {
        if ($entry.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow) {
            $identity = $entry.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
            $badInheritance = $isDirectory -and (($entry.InheritanceFlags -band $inheritance) -ne $inheritance)
            if ($identity -ne $sid.Value -or ($entry.FileSystemRights -band $rights) -ne $rights -or $badInheritance) {
                $private = $false
            } else {
                $found = $true
            }
        }
    }
    if (-not $found) { $private = $false }
    if (-not $private) {
        if ($isDirectory) {
            $acl = New-Object System.Security.AccessControl.DirectorySecurity
            $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($sid, $rights, $inheritance, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
        } else {
            $acl = New-Object System.Security.AccessControl.FileSecurity
            $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($sid, $rights, [System.Security.AccessControl.InheritanceFlags]::None, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
        }
        $acl.SetAccessRuleProtection($true, $false)
        $acl.AddAccessRule($rule)
        Set-Acl -LiteralPath $itemPath -AclObject $acl
    }
    $actual = Get-Acl -LiteralPath $itemPath
    if (-not $actual.AreAccessRulesProtected) { throw 'spill ACL is inherited' }
    $found = $false
    foreach ($entry in $actual.Access) {
        if ($entry.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow) {
            $identity = $entry.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
            if ($identity -ne $sid.Value) { throw 'spill ACL grants another identity' }
            if (($entry.FileSystemRights -band $rights) -ne $rights) { throw 'spill ACL lacks owner full control' }
            $found = $true
        }
    }
    if (-not $found) { throw 'spill ACL has no owner rule' }
}

$root = Get-Item -LiteralPath $path -Force
if (-not $root.PSIsContainer -or ($root.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'spill root is not a private directory' }
Set-PrivateAcl $root.FullName $true
foreach ($child in @(Get-ChildItem -LiteralPath $root.FullName -Force)) {
    if ($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw 'spill storage contains a reparse point' }
    if ($child.PSIsContainer) {
        Set-PrivateAcl $child.FullName $true
        foreach ($entry in @(Get-ChildItem -LiteralPath $child.FullName -Force)) {
            if ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw 'spill run contains a reparse point' }
            if ($entry.PSIsContainer) { throw 'spill run contains a nested directory' }
            Set-PrivateAcl $entry.FullName $false
        }
    } else {
        Set-PrivateAcl $child.FullName $false
    }
}
"#;

    let powershell = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut command = tokio::process::Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .env("MONA_SPILL_ACL_PATH", path.as_os_str())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(4), command.output())
        .await
        .map_err(|_| error(api::ErrorCode::Tool, "spill storage permissions timed out"))?
        .map_err(|_| error(api::ErrorCode::Tool, "spill storage permissions failed"))?;
    if !output.status.success() {
        return Err(error(
            api::ErrorCode::Tool,
            "spill storage permissions failed",
        ));
    }
    Ok(())
}
