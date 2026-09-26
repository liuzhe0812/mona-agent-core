//! Windows implementation mirrors DSH's WRITE_RESTRICTED/ACL/Low-integrity dialect.
mod acl;
mod native;
mod process;
use crate::{check_cancel, policy::is_under, CancellationToken, Error, Mode, Policy, Result};
use native::Sid;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

fn capability_sid(path: &Path, temp: bool) -> String {
    let mut hash = Sha256::new();
    if temp {
        hash.update(b"temp\0");
    }
    hash.update(path.to_string_lossy().as_bytes());
    let digest = hash.finalize();
    let part = |i| {
        u32::from_le_bytes(digest[i..i + 4].try_into().expect("SHA256 range")) % ((1 << 30) - 1) + 1
    };
    format!(
        "S-1-4-{}-{}{}",
        part(0),
        part(4),
        if temp { "-1" } else { "" }
    )
}
fn disjoint(workspace: &Path, temp: &Path) -> Result<()> {
    if is_under(temp, workspace) || is_under(workspace, temp) {
        return Err(Error::config(
            "Windows private temp must be disjoint from the workspace",
        ));
    }
    Ok(())
}
pub(crate) struct PrivateTemp {
    directory: Option<tempfile::TempDir>,
    canonical: PathBuf,
    sid: String,
}
impl PrivateTemp {
    pub fn create(workspace: &Path, cancel: &CancellationToken) -> Result<Self> {
        check_cancel(cancel)?;
        let parent = std::fs::canonicalize(std::env::temp_dir()).map_err(Error::unavailable)?;
        if is_under(&parent, workspace) {
            return Err(Error::config(
                "Windows temp root is inside the sandbox workspace",
            ));
        }
        let dir = tempfile::Builder::new()
            .prefix("mona-sandbox-")
            .tempdir_in(&parent)
            .map_err(Error::unavailable)?;
        let canonical = std::fs::canonicalize(dir.path()).map_err(Error::unavailable)?;
        disjoint(workspace, &canonical)?;
        let value = Self {
            sid: capability_sid(&canonical, true),
            canonical,
            directory: Some(dir),
        };
        // Workspace rights deliberately stand across sessions/restarts, including failure paths.
        acl::grant(
            workspace,
            &Sid::parse(&capability_sid(workspace, false))?,
            cancel,
        )?;
        acl::grant(value.path(), &Sid::parse(&value.sid)?, cancel)?;
        check_cancel(cancel)?;
        Ok(value)
    }
    pub fn path(&self) -> &Path {
        &self.canonical
    }
    fn cleanup(&mut self) -> Result<()> {
        let Some(dir) = self.directory.take() else {
            return Ok(());
        };
        let revoked = Sid::parse(&self.sid).and_then(|sid| acl::revoke(self.path(), &sid));
        let removed = dir.close().map_err(Error::unavailable);
        match (revoked, removed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(a), Err(b)) => Err(Error::unavailable(format!("{a}; {b}"))),
            (Err(e), _) | (_, Err(e)) => Err(e),
        }
    }
    pub fn close(mut self) -> Result<()> {
        self.cleanup()
    }
}
impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            eprintln!("mona-windows-acl-run: cleanup: {e}");
        }
    }
}

pub(crate) fn run(policy: &Policy, temp: Option<&Path>, argv: &[OsString]) -> Result<i32> {
    let mut writes = Vec::new();
    match policy.mode {
        Mode::ReadOnly if temp.is_none() => {}
        Mode::WorkspaceWrite => {
            let temp =
                temp.ok_or_else(|| Error::config("workspace-write runner requires private temp"))?;
            if !temp.is_dir() {
                return Err(Error::config("private temp is not an existing directory"));
            }
            let temp = std::fs::canonicalize(temp).map_err(Error::unavailable)?;
            disjoint(&policy.workspace_root, &temp)?;
            writes.push(Sid::parse(&capability_sid(&policy.workspace_root, false))?);
            writes.push(Sid::parse(&capability_sid(&temp, true))?);
            // Helper is single-threaded; never changes the parent host's environment.
            std::env::set_var("TMP", &temp);
            std::env::set_var("TEMP", &temp);
        }
        _ => {
            return Err(Error::config(
                "invalid native Windows runner mode/temp shape",
            ))
        }
    }
    let token = native::restricted_token(policy.mode, &writes)?;
    process::run(token, argv, &policy.workspace_root)
}
