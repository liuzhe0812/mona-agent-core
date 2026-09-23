//! The host chooses discovery roots; task prompts and Bridge JSON never supply filesystem authority.
use skills::{Limits, LocalSkills, SkillRegistry, SkillsPlugin};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn from_environment() -> Result<Option<SkillsPlugin>, Box<dyn std::error::Error>> {
    if std::env::var("AGENT_SKILLS").as_deref() == Ok("0") {
        return Ok(None);
    }
    let cwd = std::env::current_dir()?;
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let roots = roots(
        &cwd,
        user.as_deref(),
        std::env::var_os("AGENT_SKILL_DIRS").as_deref(),
    )?;
    let limits = Limits::default();
    let local = LocalSkills::new("filesystem", roots, limits.clone())?;
    let registry = SkillRegistry::new(vec![Arc::new(local)], limits)?;
    Ok(Some(SkillsPlugin::new(Arc::new(registry))))
}

fn roots(
    cwd: &Path,
    user: Option<&Path>,
    override_dirs: Option<&OsStr>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if let Some(value) = override_dirs {
        let roots: Vec<_> = std::env::split_paths(value).collect();
        if roots.is_empty() || roots.iter().any(|path| path.as_os_str().is_empty()) {
            return Err("AGENT_SKILL_DIRS must contain nonempty directory paths".into());
        }
        return Ok(roots);
    }
    let project = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd);
    let mut roots = vec![project.join(".agents/skills")];
    if let Some(user) = user {
        let path = user.join(".agents/skills");
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_roots_follow_project_precedence_and_explicit_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let cwd = project.join("nested");
        let user = temp.path().join("user");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(project.join(".git"), "gitdir: example").unwrap();
        assert_eq!(
            roots(&cwd, Some(&user), None).unwrap(),
            vec![project.join(".agents/skills"), user.join(".agents/skills")]
        );
        let custom = temp.path().join("isolated-skills");
        let setting = std::env::join_paths([&custom]).unwrap();
        assert_eq!(
            roots(&cwd, Some(&user), Some(&setting)).unwrap(),
            vec![custom]
        );
        assert!(roots(&cwd, Some(&user), Some(OsStr::new(""))).is_err());
    }
}
