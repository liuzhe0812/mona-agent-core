//! Read-only Git inspection for the application review pane. Never runs hooks, pagers or external diffs.
use serde::Serialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
use workspace::{error, Result};
const MAX_GIT_OUTPUT: usize = 1024 * 1024;
#[derive(Serialize)]
pub struct Change {
    pub path: String,
    pub previous_path: Option<String>,
    pub index: String,
    pub working: String,
}
#[derive(Serialize)]
pub struct Review {
    pub available: bool,
    pub message: Option<String>,
    pub branch: String,
    pub entries: Vec<Change>,
}
#[derive(Serialize)]
pub struct Diff {
    pub path: String,
    pub staged: bool,
    pub text: String,
    pub truncated: bool,
}
fn io(_: impl std::fmt::Display) -> workspace::Error {
    error("io", "Git 读取失败；请确认 Git 已安装且目录可访问。")
}
async fn git(root: &Path, args: &[&str]) -> Result<(bool, Vec<u8>)> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=false",
        ])
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let operation = async move {
        let mut child = command.spawn().map_err(io)?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| io("stdout"))?
            .take((MAX_GIT_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(io)?;
        if bytes.len() > MAX_GIT_OUTPUT {
            let _ = child.kill().await;
            return Err(error(
                "capacity",
                "Git 结果超过 1 MiB，请选择单个文件查看。",
            ));
        }
        let ok = child.wait().await.map_err(io)?.success();
        Ok((ok, bytes))
    };
    tokio::time::timeout(Duration::from_secs(10), operation)
        .await
        .map_err(|_| error("busy", "Git 读取超时，未执行任何修改。"))?
}
fn relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.starts_with('/')
        && !path.contains(['\\', ':', '\0'])
        && path
            .split('/')
            .all(|p| !matches!(p, "" | "." | ".." | ".git"))
}
fn permitted(
    directory: &workspace::Directory,
    private: &[std::path::PathBuf],
    path: &str,
    parents: &mut std::collections::HashMap<String, bool>,
) -> bool {
    if !relative(path) {
        return false;
    }
    let absolute = directory.root().join(path);
    if private
        .iter()
        .any(|p| workspace::contains_path(p, &absolute))
    {
        return false;
    }
    // Authorize the parent through the existing no-link resolver without reading every file body.
    let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let allowed = *parents
        .entry(parent.into())
        .or_insert_with(|| directory.list(parent, 0, 1, None).is_ok());
    if !allowed {
        return false;
    }
    match std::fs::symlink_metadata(&absolute) {
        Ok(meta) => meta.is_file() && !workspace::is_link(&meta),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}
async fn root_matches(directory: &workspace::Directory) -> Result<bool> {
    let (ok, bytes) = git(directory.root(), &["rev-parse", "--show-toplevel"]).await?;
    if !ok {
        return Ok(false);
    }
    let path = String::from_utf8(bytes).map_err(io)?;
    Ok(std::fs::canonicalize(path.trim())
        .map(|root| workspace::same_path(&root, directory.root()))
        .unwrap_or(false))
}
pub async fn list(
    directory: workspace::Directory,
    private: Vec<std::path::PathBuf>,
) -> Result<Review> {
    if !root_matches(&directory).await? {
        return Ok(Review {
            available: false,
            message: Some("当前工作目录不是 Git 仓库根目录。".into()),
            branch: String::new(),
            entries: Vec::new(),
        });
    }
    let (_, branch) = git(
        directory.root(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .await?;
    let (ok, bytes) = git(
        directory.root(),
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ],
    )
    .await?;
    if !ok {
        return Err(io("status"));
    }
    tokio::task::spawn_blocking(move || {
        let text = String::from_utf8(bytes).map_err(io)?;
        let mut parts = text.split('\0');
        let mut entries = Vec::new();
        let mut parents = std::collections::HashMap::new();
        while let Some(part) = parts.next() {
            if part.len() < 4 {
                continue;
            }
            let index = part.chars().next().unwrap();
            let working = part.chars().nth(1).unwrap();
            let path = &part[3..];
            let previous = if matches!(index, 'R' | 'C') || matches!(working, 'R' | 'C') {
                parts.next().map(str::to_owned)
            } else {
                None
            };
            if path.ends_with('/')
                || !permitted(&directory, &private, path, &mut parents)
                || previous
                    .as_ref()
                    .is_some_and(|p| !permitted(&directory, &private, p, &mut parents))
            {
                continue;
            }
            entries.push(Change {
                path: path.into(),
                previous_path: previous,
                index: index.to_string(),
                working: working.to_string(),
            });
            if entries.len() > 1000 {
                return Err(error("capacity", "变更文件超过 1000 个，请缩小工作区。"));
            }
        }
        Ok(Review {
            available: true,
            message: None,
            branch: String::from_utf8(branch).map_err(io)?.trim().into(),
            entries,
        })
    })
    .await
    .map_err(io)?
}
pub async fn diff(
    directory: workspace::Directory,
    private: Vec<std::path::PathBuf>,
    path: &str,
    staged: bool,
) -> Result<Diff> {
    if !permitted(
        &directory,
        &private,
        path,
        &mut std::collections::HashMap::new(),
    ) || !root_matches(&directory).await?
    {
        return Err(error("forbidden", "此文件不允许通过审查面板读取。"));
    }
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--ignore-submodules=all",
    ];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", path]);
    let (ok, bytes) = git(directory.root(), &args).await?;
    if !ok {
        return Err(io("diff"));
    }
    let mut text = String::from_utf8(bytes).map_err(io)?;
    let mut truncated = false;
    if text.is_empty() && !staged {
        let (tracked, _) = git(
            directory.root(),
            &["ls-files", "--error-unmatch", "--", path],
        )
        .await?;
        if !tracked {
            let file = directory.read(path, 0, 65536, None)?;
            truncated = !file.eof;
            text = file
                .text
                .map(|body| {
                    format!(
                        "--- /dev/null\n+++ {path}\n{}",
                        body.lines()
                            .map(|line| format!("+{line}\n"))
                            .collect::<String>()
                    )
                })
                .unwrap_or_else(|| "二进制文件，无法显示文本差异。".into());
        }
    }
    Ok(Diff {
        path: path.into(),
        staged,
        text,
        truncated,
    })
}
