use std::{env, path::PathBuf, sync::Arc};

pub fn from_environment(
    read_extensions: Vec<Arc<dyn tools::ReadExtension>>,
) -> Result<tools::ToolConfig, Box<dyn std::error::Error>> {
    let cwd = match env::var_os("AGENT_WORKSPACE_DIR") {
        Some(value) => PathBuf::from(value),
        None => env::current_dir()?,
    };
    if !cwd.is_dir() {
        return Err(format!("Agent workspace is not a directory: {}", cwd.display()).into());
    }
    let shell = match env::var_os("AGENT_SHELL_PATH") {
        Some(value) => tools::ShellConfig::from(require_file(PathBuf::from(value))?),
        None => match env::var_os("AGENT_BASH_PATH") {
            Some(value) => {
                tools::ShellConfig::new(require_file(PathBuf::from(value))?, tools::ShellKind::Bash)
            }
            None => tools::ShellConfig::discover()?,
        },
    };
    let mut config = tools::ToolConfig::new(cwd, shell);
    config.read_extensions = read_extensions;
    Ok(config)
}

fn require_file(path: PathBuf) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("Shell executable not found: {}", path.display()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_shell_is_validated_and_native_shell_is_discovered() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join(if cfg!(windows) {
            "powershell.exe"
        } else {
            "sh"
        });
        std::fs::write(&executable, "fixture").unwrap();
        assert_eq!(require_file(executable.clone()).unwrap(), executable);
        assert!(require_file(directory.path().join("missing")).is_err());
        let native = tools::ShellConfig::discover().unwrap();
        assert!(native.executable.is_file());
        #[cfg(windows)]
        assert_eq!(native.kind, tools::ShellKind::PowerShell);
    }
}
