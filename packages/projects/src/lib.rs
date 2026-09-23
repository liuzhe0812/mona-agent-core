//! Optional project registration. Directories and sessions are never deleted by this registry.
#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use workspace::{canonical_root, error, path_text, same_path, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    request_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub version: u32,
    pub revision: u64,
    pub projects: Vec<Project>,
}
pub struct Registry {
    path: PathBuf,
    data: Mutex<Listing>,
    _lease: File,
}
fn io(_: impl std::fmt::Display) -> workspace::Error {
    error("io", "项目登记保存或读取失败，原记录未被覆盖。")
}
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| error("invalid_request", "项目配置路径无效。"))?;
        fs::create_dir_all(parent).map_err(io)?;
        for p in [path.to_owned(), path.with_extension("lock")] {
            if let Ok(m) = fs::symlink_metadata(&p) {
                if workspace::is_link(&m) || !m.is_file() {
                    return Err(error("forbidden", "项目状态不能是链接或非普通文件。"));
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
            .map_err(|_| error("conflict", "项目登记已由其他宿主占用。"))?;
        let data = match File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io)?;
                if bytes.len() > 1024 * 1024 {
                    return Err(error("capacity", "项目登记文件过大。"));
                }
                let data: Listing = serde_json::from_slice(&bytes)
                    .map_err(|_| error("invalid_request", "项目登记格式损坏，未重置原文件。"))?;
                if data.version != 1 || data.projects.len() > 256 {
                    return Err(error("invalid_request", "不支持的项目登记格式。"));
                }
                let mut ids = std::collections::HashSet::new();
                for p in &data.projects {
                    if !valid_key(&p.id)
                        || !valid_key(&p.request_id)
                        || !Path::new(&p.path).is_absolute()
                        || p.name.trim().is_empty()
                        || p.name.len() > 320
                        || !ids.insert(&p.id)
                    {
                        return Err(error("invalid_request", "项目登记内容无效。"));
                    }
                }
                data
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Listing {
                version: 1,
                revision: 0,
                projects: vec![],
            },
            Err(e) => return Err(io(e)),
        };
        Ok(Self {
            path: path.into(),
            data: Mutex::new(data),
            _lease: lease,
        })
    }
    pub fn list(&self) -> Result<Listing> {
        Ok(self.data.lock().map_err(io)?.clone())
    }
    pub fn get(&self, id: &str) -> Result<Project> {
        self.data
            .lock()
            .map_err(io)?
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| error("not_found", "项目已移除或不存在。"))
    }
    fn save(&self, next: &Listing) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap()).map_err(io)?;
        serde_json::to_writer(file.as_file_mut(), next).map_err(io)?;
        file.flush().map_err(io)?;
        file.as_file().sync_all().map_err(io)?;
        file.persist(&self.path)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        #[cfg(unix)]
        File::open(self.path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        Ok(())
    }
    pub fn add(&self, key: &str, revision: u64, name: &str, path: &Path) -> Result<Listing> {
        if !valid_key(key) {
            return Err(error("invalid_request", "项目请求 ID 无效。"));
        }
        let root = canonical_root(path)?;
        let name = name.trim();
        if name.is_empty()
            || name.len() > 320
            || name.chars().count() > 80
            || name.chars().any(char::is_control)
        {
            return Err(error("invalid_request", "项目名称需要 1–80 个字符。"));
        }
        let mut data = self.data.lock().map_err(io)?;
        if let Some(p) = data.projects.iter().find(|p| p.request_id == key) {
            return if same_path(Path::new(&p.path), &root) && p.name == name {
                Ok(data.clone())
            } else {
                Err(error("conflict", "同一请求 ID 的项目参数不同。"))
            };
        }
        if data.revision != revision {
            return Err(error("conflict", "项目列表已更新，请刷新。"));
        }
        if data
            .projects
            .iter()
            .any(|p| same_path(Path::new(&p.path), &root))
        {
            return Err(error("conflict", "此目录已经登记为项目。"));
        }
        if data.projects.len() >= 256 {
            return Err(error("capacity", "项目数量达到 256 个上限。"));
        }
        let mut next = data.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| error("capacity", "项目版本超限。"))?;
        // The same request key after removal must not reassign old sessions to a new project.
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{key}:{}", next.revision).as_bytes())
        );
        next.projects.push(Project {
            id: format!("p-{}", &hash[..32]),
            name: name.into(),
            path: path_text(&root)?,
            request_id: key.into(),
        });
        self.save(&next)?;
        *data = next;
        Ok(data.clone())
    }
    pub fn remove(&self, id: &str, revision: u64) -> Result<Listing> {
        let mut data = self.data.lock().map_err(io)?;
        if data.revision != revision {
            return Err(error("conflict", "项目列表已更新，请刷新。"));
        }
        if !data.projects.iter().any(|p| p.id == id) {
            return Err(error("not_found", "项目不存在。"));
        }
        let mut next = data.clone();
        next.projects.retain(|p| p.id != id);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| error("capacity", "项目版本超限。"))?;
        self.save(&next)?;
        *data = next;
        Ok(data.clone())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_removed_project_identity_is_never_reused_for_another_registration() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let registry = Registry::open(&t.path().join("state/projects.json")).unwrap();
        let first = registry.add("request", 0, "first", &a).unwrap();
        let id = &first.projects[0].id;
        let removed = registry.remove(id, first.revision).unwrap();
        let next = registry
            .add("request", removed.revision, "second", &b)
            .unwrap();
        assert_ne!(*id, next.projects[0].id);
        assert_eq!(
            registry
                .add("request", removed.revision, "second", &b)
                .unwrap()
                .revision,
            next.revision
        );
    }
    #[test]
    fn registration_is_atomic_idempotent_and_never_removes_files() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("work");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("keep"), "data").unwrap();
        let path = t.path().join("private/projects.json");
        let id;
        {
            let r = Registry::open(&path).unwrap();
            let a = r.add("first", 0, "Project", &dir).unwrap();
            id = a.projects[0].id.clone();
            assert_eq!(r.add("first", 0, "Project", &dir).unwrap().revision, 1);
            assert!(r.add("second", 0, "X", &dir).is_err());
            assert!(Registry::open(&path).is_err());
        }
        let r = Registry::open(&path).unwrap();
        assert_eq!(r.get(&id).unwrap().name, "Project");
        r.remove(&id, 1).unwrap();
        assert!(dir.join("keep").exists());
    }
}
