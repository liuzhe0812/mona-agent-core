//! Bounded, root-scoped file inspection. No project registry, model or execution loop.
#![forbid(unsafe_code)]
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_PAGE: usize = 200;
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub fn error(code: &str, message: &str) -> Error {
    Error {
        code: code.into(),
        message: message.into(),
    }
}
fn io(e: std::io::Error) -> Error {
    let (code, message) = match e.kind() {
        std::io::ErrorKind::NotFound => ("not_found", "目录或文件不存在。"),
        std::io::ErrorKind::PermissionDenied => ("forbidden", "没有访问目录或文件的权限。"),
        _ => ("io", "文件访问失败，请检查目录权限及磁盘状态。"),
    };
    error(code, message)
}
pub fn canonical_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(error("invalid_request", "工作目录必须是绝对路径。"));
    }
    let root = fs::canonicalize(path).map_err(io)?;
    if !fs::metadata(&root).map_err(io)?.is_dir() {
        return Err(error("invalid_request", "工作目录不是文件夹。"));
    }
    Ok(root)
}
pub fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("unsupported", "路径不是有效 Unicode。"))
}
pub fn same_path(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
/// Component-aware containment, including Windows case-insensitive path spelling.
/// Inputs are absolute/canonical paths supplied by the host, not authorization tokens.
pub fn contains_path(root: &Path, candidate: &Path) -> bool {
    let mut parts = candidate.components();
    root.components().all(|part| {
        parts
            .next()
            .is_some_and(|next| same_path(Path::new(part.as_os_str()), Path::new(next.as_os_str())))
    })
}
pub fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn stamp(meta: &fs::Metadata) -> String {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return format!("{}:{}:{}:{}", meta.dev(), meta.ino(), meta.len(), modified);
    }
    #[cfg(not(unix))]
    {
        format!("{}:{}:{:?}", meta.len(), modified, meta.created().ok())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn check_revision(expected: Option<&str>, actual: &str) -> Result<()> {
    if expected.is_some_and(|v| v != actual) {
        return Err(error("conflict", "文件或目录已变化，请刷新后重新读取。"));
    }
    Ok(())
}

#[derive(Clone)]
pub struct Directory {
    root: PathBuf,
    excluded: Vec<PathBuf>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub bytes: u64,
}
#[derive(Debug, Serialize)]
pub struct DirectoryPage {
    pub path: String,
    pub entries: Vec<Entry>,
    pub revision: String,
    pub next_offset: Option<usize>,
}
#[derive(Debug, Serialize)]
pub struct FilePage {
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub media_type: String,
    pub revision: String,
    pub offset: usize,
    pub next_offset: usize,
    pub eof: bool,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}
impl Directory {
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            root: canonical_root(root)?,
            excluded: Vec::new(),
        })
    }
    /// Exclusions are trusted canonical private-state roots, not browser input.
    pub fn excluding(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.excluded
            .extend(roots.into_iter().map(|p| fs::canonicalize(&p).unwrap_or(p)));
        self
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn denied(&self, path: &Path) -> bool {
        self.excluded.iter().any(|p| contains_path(p, path))
    }
    fn resolve(&self, relative: &str) -> Result<PathBuf> {
        if relative.len() > 4096
            || relative.contains(['\0', '\\', ':'])
            || relative.starts_with('/')
        {
            return Err(error("invalid_request", "只允许工作目录内的相对路径。"));
        }
        let mut path = self.root.clone();
        if self.denied(&path) || is_link(&fs::symlink_metadata(&path).map_err(io)?) {
            return Err(error("forbidden", "此目录不能通过文件浏览访问。"));
        }
        if !same_path(&fs::canonicalize(&path).map_err(io)?, &self.root) {
            return Err(error("conflict", "工作目录已被替换，请重新确认。"));
        }
        for c in Path::new(relative).components() {
            match c {
                Component::CurDir if relative == "." => {}
                Component::Normal(name) => {
                    path.push(name);
                    let meta = fs::symlink_metadata(&path).map_err(io)?;
                    if is_link(&meta) || self.denied(&path) {
                        return Err(error("forbidden", "不允许读取链接或宿主私有目录。"));
                    }
                }
                _ => return Err(error("invalid_request", "不允许跳出当前工作目录。")),
            }
        }
        let canonical = fs::canonicalize(&path).map_err(io)?;
        if !contains_path(&self.root, &canonical) || self.denied(&canonical) {
            return Err(error("forbidden", "路径不在允许的工作目录范围内。"));
        }
        Ok(path)
    }
    /// Cheap authorized metadata for UI file references; never loads or hashes file contents.
    /// Opening/reading the file must still revalidate the path and content revision.
    pub fn stat(&self, relative: &str) -> Result<Entry> {
        let path = self.resolve(relative)?;
        let meta = fs::symlink_metadata(&path).map_err(io)?;
        if is_link(&meta) {
            return Err(error("forbidden", "不允许读取链接或宿主私有目录。"));
        }
        let kind = if meta.is_file() {
            "file"
        } else if meta.is_dir() {
            "directory"
        } else {
            "other"
        };
        Ok(Entry {
            name: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned(),
            path: relative.to_owned(),
            kind: kind.into(),
            bytes: meta.len(),
        })
    }
    pub fn list(
        &self,
        relative: &str,
        offset: usize,
        limit: usize,
        revision: Option<&str>,
    ) -> Result<DirectoryPage> {
        if !(1..=MAX_PAGE).contains(&limit) || offset > MAX_ENTRIES {
            return Err(error("invalid_request", "目录分页参数超限。"));
        }
        let path = self.resolve(relative)?;
        if !fs::metadata(&path).map_err(io)?.is_dir() {
            return Err(error("invalid_request", "不是文件夹。"));
        }
        let mut values = Vec::new();
        for (count, entry) in fs::read_dir(&path).map_err(io)?.enumerate() {
            if count >= MAX_ENTRIES {
                return Err(error("capacity", "目录超过 10000 项，请选择较小的子目录。"));
            }
            let entry = entry.map_err(io)?;
            if self.denied(&entry.path()) {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| error("unsupported", "目录包含无法显示的非 Unicode 文件名。"))?;
            let meta = fs::symlink_metadata(entry.path()).map_err(io)?;
            let kind = if is_link(&meta) {
                "link"
            } else if meta.is_dir() {
                "directory"
            } else if meta.is_file() {
                "file"
            } else {
                "other"
            };
            let prefix = relative.trim_matches('/').trim_start_matches("./");
            let child = if prefix.is_empty() || prefix == "." {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            values.push((
                Entry {
                    name,
                    path: child,
                    kind: kind.into(),
                    bytes: meta.len(),
                },
                stamp(&meta),
            ));
        }
        self.resolve(relative)?;
        values.sort_by(|a, b| {
            (a.0.kind != "directory", a.0.name.to_lowercase(), &a.0.name).cmp(&(
                b.0.kind != "directory",
                b.0.name.to_lowercase(),
                &b.0.name,
            ))
        });
        let encoded = serde_json::to_vec(&values).map_err(|_| error("io", "目录信息编码失败。"))?;
        let current = digest(&encoded);
        check_revision(revision, &current)?;
        let next = offset.saturating_add(limit);
        let next_offset = (next < values.len()).then_some(next);
        Ok(DirectoryPage {
            path: relative.into(),
            entries: values
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|v| v.0)
                .collect(),
            revision: current,
            next_offset,
        })
    }
    pub fn read(
        &self,
        relative: &str,
        offset: usize,
        limit: usize,
        revision: Option<&str>,
    ) -> Result<FilePage> {
        if !(4..=MAX_PAGE_BYTES).contains(&limit) || offset > MAX_TEXT_BYTES {
            return Err(error("invalid_request", "文件分页参数超限。"));
        }
        let path = self.resolve(relative)?;
        let before = fs::metadata(&path).map_err(io)?;
        if !before.is_file() {
            return Err(error("unsupported", "只允许预览普通文件。"));
        }
        if before.len() > MAX_TEXT_BYTES as u64 {
            return Err(error("capacity", "文件超过 16 MiB 预览限制。"));
        }
        let file = fs::File::open(&path).map_err(io)?;
        if stamp(&file.metadata().map_err(io)?) != stamp(&before) {
            return Err(error("conflict", "文件读取前已变化。"));
        }
        let mut bytes = Vec::with_capacity(before.len() as usize);
        file.take(MAX_TEXT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > MAX_TEXT_BYTES {
            return Err(error("capacity", "文件预览超过容量限制。"));
        }
        self.resolve(relative)?;
        if stamp(&fs::metadata(&path).map_err(io)?) != stamp(&before) {
            return Err(error("conflict", "文件读取期间已变化，请刷新。"));
        }
        let current = digest(&bytes);
        check_revision(revision, &current)?;
        let image = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some("image/jpeg")
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some("image/gif")
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Some("image/webp")
        } else {
            None
        };
        let mut page = FilePage {
            path: relative.into(),
            name: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .into(),
            bytes: bytes.len() as u64,
            media_type: "application/octet-stream".into(),
            revision: current,
            offset,
            next_offset: bytes.len(),
            eof: true,
            kind: "binary".into(),
            text: None,
            base64: None,
        };
        if let Some(media) = image {
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err(error("capacity", "图片超过 4 MiB 预览限制。"));
            }
            if offset != 0 {
                return Err(error("invalid_request", "图片不使用文本分页。"));
            }
            page.media_type = media.into();
            page.kind = "image".into();
            page.base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
        } else if !bytes.contains(&0) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                if offset > bytes.len() || !text.is_char_boundary(offset) {
                    return Err(error("invalid_request", "读取位置不是有效的 UTF-8 边界。"));
                }
                let mut end = offset.saturating_add(limit).min(bytes.len());
                while end > offset && !text.is_char_boundary(end) {
                    end -= 1;
                }
                page.kind = "text".into();
                page.media_type = "text/plain".into();
                page.text = Some(text[offset..end].into());
                page.next_offset = end;
                page.eof = end == bytes.len();
            }
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_are_scoped_versioned_and_unicode_safe() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("中文.txt"), "中午abc").unwrap();
        let dir = Directory::open(root.path()).unwrap();
        let listing = dir.list("", 0, 10, None).unwrap();
        assert_eq!(listing.entries[0].name, "中文.txt");
        let first = dir.read("中文.txt", 0, 4, None).unwrap();
        assert_eq!(first.text.as_deref(), Some("中"));
        let second = dir
            .read("中文.txt", first.next_offset, 4, Some(&first.revision))
            .unwrap();
        assert_eq!(second.text.as_deref(), Some("午a"));
        fs::write(root.path().join("中文.txt"), "新版本").unwrap();
        assert_eq!(
            dir.read("中文.txt", 0, 64, Some(&first.revision))
                .unwrap_err()
                .code,
            "conflict"
        );
        assert!(dir.list("", 0, 10, Some(&listing.revision)).is_err());
        for p in ["../x", "/etc/passwd", "C:/private", "a\\b", "x:stream"] {
            assert!(dir.read(p, 0, 64, None).is_err(), "{p}");
        }
    }
    #[test]
    fn exclusions_and_binary_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("state");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("secret"), "key").unwrap();
        fs::write(root.path().join("data.bin"), [0, 1, 2]).unwrap();
        let dir = Directory::open(root.path()).unwrap().excluding([private]);
        assert_eq!(dir.list("", 0, 20, None).unwrap().entries.len(), 1);
        assert!(dir.read("state/secret", 0, 64, None).is_err());
        assert_eq!(dir.read("data.bin", 0, 64, None).unwrap().kind, "binary");
    }
    #[test]
    fn metadata_is_authorized_and_does_not_require_reading_a_preview() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("state");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("secret"), "key").unwrap();
        let file = fs::File::create(root.path().join("large.bin")).unwrap();
        file.set_len(20 * 1024 * 1024).unwrap();
        drop(file);
        let dir = Directory::open(root.path()).unwrap().excluding([private]);
        let entry = dir.stat("large.bin").unwrap();
        assert_eq!(entry.kind, "file");
        assert_eq!(entry.name, "large.bin");
        assert_eq!(entry.bytes, 20 * 1024 * 1024);
        assert!(dir.read("large.bin", 0, 4, None).is_err());
        assert_eq!(dir.stat("state/secret").unwrap_err().code, "forbidden");
        for path in ["../secret", "/etc/passwd", "C:/private", "x:stream", "x\0y"] {
            assert!(dir.stat(path).is_err(), "{path:?}");
        }
        assert_eq!(dir.stat("missing.txt").unwrap_err().code, "not_found");
    }
    #[test]
    fn containment_uses_components_and_windows_case_rules() {
        assert!(contains_path(
            Path::new("/a/state"),
            Path::new("/a/state/file")
        ));
        assert!(!contains_path(
            Path::new("/a/state"),
            Path::new("/a/state-other/file")
        ));
        #[cfg(windows)]
        {
            assert!(contains_path(
                Path::new(r"C:\Private\State"),
                Path::new(r"c:\private\state\secret")
            ));
            assert!(!contains_path(
                Path::new(r"C:\Private\State"),
                Path::new(r"c:\private\stateful\secret")
            ));
        }
    }
    #[cfg(windows)]
    #[test]
    fn differently_cased_private_directory_is_still_excluded() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("PRIVATE_STATE");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("secret"), "key").unwrap();
        let directory = Directory::open(root.path()).unwrap().excluding([private]);
        assert_eq!(
            directory
                .read("private_state/secret", 0, 64, None)
                .unwrap_err()
                .code,
            "forbidden"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_is_visible_but_never_followed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let dir = Directory::open(root.path()).unwrap();
        assert_eq!(dir.list("", 0, 10, None).unwrap().entries[0].kind, "link");
        assert!(dir.read("link/secret", 0, 64, None).is_err());
        assert!(dir.stat("link/secret").is_err());
    }
}
