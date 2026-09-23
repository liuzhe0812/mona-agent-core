//! Deployment settings only. Reusable root selection and assembly live in skills.
use skills::{discovery_roots, Limits, SkillsPlugin};
use std::path::{Path, PathBuf};

pub fn from_environment(workspace: &Path) -> Result<Option<SkillsPlugin>, Box<dyn std::error::Error>> {
    if std::env::var("AGENT_SKILLS").as_deref() == Ok("0") {
        return Ok(None);
    }
    let workspace = std::fs::canonicalize(workspace)?;
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let explicit = std::env::var_os("AGENT_SKILL_DIRS")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>());
    let roots = discovery_roots(&workspace, user.as_deref(), explicit.as_deref())?;
    Ok(Some(SkillsPlugin::local(roots, Limits::default())?))
}
