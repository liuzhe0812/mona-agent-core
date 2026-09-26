//! Actual OS enforcement. No simulated backend, no successful skip when confinement is missing.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod native {
    use sandbox::{CancellationToken, Enforcement, LocalSandbox, Mode, Policy, Runner};
    use std::{
        ffi::OsString,
        path::Path,
        process::{Output, Stdio},
        time::Duration,
    };

    fn provider() -> LocalSandbox {
        LocalSandbox::new(Runner::standalone(env!("CARGO_BIN_EXE_sandbox-run")))
    }
    fn argv(script: &str) -> Vec<OsString> {
        #[cfg(windows)]
        {
            let shell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            vec![
                shell.into_os_string(),
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("$ErrorActionPreference='Stop'; {script}").into(),
            ]
        }
        #[cfg(not(windows))]
        {
            vec!["/bin/sh".into(), "-c".into(), script.into()]
        }
    }
    async fn execute(provider: &LocalSandbox, policy: &Policy, script: &str) -> Output {
        let plan = provider
            .prepare(&argv(script), policy, &CancellationToken::new())
            .await
            .unwrap();
        #[cfg(windows)]
        assert_eq!(plan.info.unwrap().enforcement, Enforcement::Partial);
        #[cfg(not(windows))]
        let _ = Enforcement::Full;
        let mut c = tokio::process::Command::new(&plan.program);
        c.args(&plan.args)
            .current_dir(&policy.workspace_root)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        c.creation_flags(0x0800_0000);
        let output = tokio::time::timeout(Duration::from_secs(20), c.output())
            .await
            .expect("native process timeout")
            .unwrap();
        eprintln!(
            "native exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn redirect(path: &Path) -> String {
        if cfg!(windows) {
            format!(
                "Set-Content -LiteralPath '{}' -Value granted -ErrorAction Stop",
                path.display().to_string().replace('\'', "''")
            )
        } else {
            format!(
                "echo granted > '{}'",
                path.display().to_string().replace('\'', "'\\''")
            )
        }
    }
    #[tokio::test]
    async fn real_process_write_boundary_downgrade_reads_and_private_temp_cleanup() {
        // /tmp is intentionally writable in DSH's Linux/Seatbelt workspace mode.
        // A sibling denial probe must therefore live outside that allowed temporary area.
        // Windows label grants require WRITE_OWNER, which stock data-volume Modify
        // ACLs do not supply. Use an isolated user-owned root, not weaker confinement
        // or an elevated test process. It must also be outside the writable temp root.
        #[cfg(windows)]
        let base = std::path::PathBuf::from(
            std::env::var_os("LOCALAPPDATA")
                .expect("native sandbox tests require the user's LOCALAPPDATA directory"),
        )
        .join("mona-agent-core/sandbox-test-workspaces");
        #[cfg(not(windows))]
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/sandbox-native");
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::tempdir_in(&base).unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "OUTSIDE_READABLE").unwrap();
        let provider = provider();
        let rw = Policy::new(Mode::WorkspaceWrite, &workspace)
            .unwrap()
            .with_session("one")
            .unwrap();
        let inside = workspace.join("inside.txt");
        std::fs::write(&inside, "pre-existing project file").unwrap();
        let result = execute(&provider, &rw, &redirect(&inside)).await;
        assert!(result.status.success());
        assert!(inside.exists());
        let result = execute(&provider, &rw, &redirect(&outside)).await;
        assert!(!result.status.success());
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "OUTSIDE_READABLE"
        );
        let ro = Policy::new(Mode::ReadOnly, &workspace)
            .unwrap()
            .with_session("one")
            .unwrap();
        let result = execute(&provider, &ro, &redirect(&workspace.join("readonly.txt"))).await;
        assert!(!result.status.success());
        assert!(!workspace.join("readonly.txt").exists());
        let read = if cfg!(windows) {
            format!(
                "Get-Content -LiteralPath '{}' -Raw -ErrorAction Stop",
                outside.display().to_string().replace('\'', "''")
            )
        } else {
            format!("cat '{}'", outside.display())
        };
        let result = execute(&provider, &ro, &read).await;
        assert!(result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains("OUTSIDE_READABLE"));
        #[cfg(windows)]
        {
            let unicode = execute(&provider, &ro, "'READ_ONLY_中文'").await;
            assert!(unicode.status.success());
            assert!(String::from_utf8_lossy(&unicode.stdout).contains("READ_ONLY_中文"));
        }
        #[cfg(windows)]
        {
            let first = execute(&provider, &rw, "$env:TEMP").await;
            assert!(first.status.success());
            let temp1 = std::path::PathBuf::from(String::from_utf8_lossy(&first.stdout).trim());
            assert!(temp1.is_dir());
            let other = Policy::new(Mode::WorkspaceWrite, &workspace)
                .unwrap()
                .with_session("two")
                .unwrap();
            let second = execute(&provider, &other, "$env:TEMP").await;
            assert!(second.status.success());
            let temp2 = std::path::PathBuf::from(String::from_utf8_lossy(&second.stdout).trim());
            assert_ne!(temp1, temp2);
            assert!(!execute(
                &provider,
                &other,
                &redirect(&temp1.join("must-not-write.txt"))
            )
            .await
            .status
            .success());
            provider.release_session("one").await.unwrap();
            assert!(!temp1.exists() && temp2.exists());
            provider.shutdown().await.unwrap();
            assert!(!temp1.exists() && !temp2.exists());
        }
        #[cfg(not(windows))]
        provider.shutdown().await.unwrap();
        assert!(provider
            .prepare(&argv("echo must-not-run"), &rw, &CancellationToken::new())
            .await
            .is_err());
    }
}
