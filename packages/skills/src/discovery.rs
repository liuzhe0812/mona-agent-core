//! Conventional discovery is opt-in. A host may instead pass exact roots to LocalSkills.
use crate::{error, Limits, LocalSkills, SkillRegistry, SkillsPlugin};
use api::{ErrorCode, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Select roots from the actual workspace, never from the process cwd or user environment.
/// Explicit roots replace both conventional roots; relative overrides resolve from workspace.
/// A .git file (worktree) and a .git directory are both project markers.
pub fn discovery_roots(
    workspace: &Path,
    user: Option<&Path>,
    explicit: Option<&[PathBuf]>,
) -> Result<Vec<PathBuf>> {
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err(error(
            ErrorCode::Configuration,
            "skill workspace must be an existing absolute directory",
        ));
    }
    if let Some(explicit) = explicit {
        if explicit.is_empty() || explicit.iter().any(|p| p.as_os_str().is_empty()) {
            return Err(error(
                ErrorCode::Configuration,
                "explicit skill roots must contain nonempty paths",
            ));
        }
        let mut roots = Vec::new();
        for path in explicit {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                workspace.join(path)
            };
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
        return Ok(roots);
    }
    let project = workspace
        .ancestors()
        .find(|p| p.join(".git").exists())
        .unwrap_or(workspace);
    let mut roots = vec![project.join(".agents/skills")];
    if let Some(user) = user {
        if !user.is_absolute() {
            return Err(error(
                ErrorCode::Configuration,
                "skill user directory must be absolute",
            ));
        }
        let path = user.join(".agents/skills");
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    Ok(roots)
}
impl SkillsPlugin {
    /// Small filesystem assembly helper. Custom providers still use SkillRegistry::new.
    pub fn local(roots: Vec<PathBuf>, limits: Limits) -> Result<Self> {
        let local = LocalSkills::new("filesystem", roots, limits.clone())?;
        Ok(Self::new(Arc::new(SkillRegistry::new(
            vec![Arc::new(local)],
            limits,
        )?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_roots_preserve_precedence_and_explicit_isolation_without_environment() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let workspace = project.join("nested");
        let user = temp.path().join("user");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(project.join(".git"), "gitdir: fixture").unwrap();
        assert_eq!(
            discovery_roots(&workspace, Some(&user), None).unwrap(),
            vec![project.join(".agents/skills"), user.join(".agents/skills")]
        );
        assert_eq!(
            discovery_roots(&workspace, Some(&project), None).unwrap(),
            vec![project.join(".agents/skills")]
        );
        let custom = vec![
            PathBuf::from("private-skills"),
            PathBuf::from("private-skills"),
        ];
        assert_eq!(
            discovery_roots(&workspace, Some(&user), Some(&custom)).unwrap(),
            vec![workspace.join("private-skills")]
        );
        assert!(discovery_roots(&workspace, None, Some(&[])).is_err());
        assert!(discovery_roots(&workspace, None, Some(&[PathBuf::new()])).is_err());
        assert!(discovery_roots(Path::new("relative"), None, None).is_err());
    }
}
