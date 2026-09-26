//! Pure argv/profile builders matching DSH sandbox-local, not shell-string interpolation.
use crate::{Mode, Policy, Result};
use std::ffi::OsString;

pub fn bubblewrap(policy: &Policy) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--die-with-parent",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if policy.mode == Mode::WorkspaceWrite {
        args.extend([
            "--tmpfs".into(),
            "/tmp".into(),
            "--bind".into(),
            policy.workspace_root.clone().into_os_string(),
            policy.workspace_root.clone().into_os_string(),
        ]);
    }
    args
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
pub fn seatbelt(policy: &Policy) -> Result<String> {
    let mut profile = String::from("(version 1) (allow default) (deny file-write*) (allow file-write* (literal \"/dev/null\"))");
    let roots = policy.writable_roots()?;
    if !roots.is_empty() {
        profile.push_str(" (allow file-write*");
        for root in roots {
            profile.push_str(&format!(
                " (subpath {})",
                quote(
                    root.to_str()
                        .ok_or_else(|| crate::Error::config("Seatbelt path is not UTF-8"))?
                )
            ));
        }
        profile.push(')');
    }
    Ok(profile)
}
