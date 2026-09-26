use sandbox::{
    canonical_target, profiles, Backend, CancellationToken, Diagnostic, Diagnostics, ErrorCode,
    LocalSandbox, Mode, Policy, Runner,
};
use std::{ffi::OsString, path::PathBuf};

#[test]
fn modes_are_exact_and_default_is_read_only() {
    assert_eq!(Mode::default(), Mode::ReadOnly);
    for mode in [Mode::ReadOnly, Mode::WorkspaceWrite, Mode::DangerFullAccess] {
        assert_eq!(mode.as_str().parse::<Mode>().unwrap(), mode);
        assert_eq!(
            serde_json::from_str::<Mode>(&serde_json::to_string(&mode).unwrap()).unwrap(),
            mode
        );
    }
    for text in ["", "read_only", "unsafe", "workspace-write "] {
        assert!(text.parse::<Mode>().is_err());
    }
}
#[test]
fn policy_fences_writes_with_canonical_targets_and_never_creates_files() {
    let root = tempfile::tempdir().unwrap();
    let readonly = Policy::new(Mode::ReadOnly, root.path()).unwrap();
    let target = root.path().join("new/sub/file.txt");
    assert_eq!(
        readonly.check_write(&target).unwrap_err().code,
        ErrorCode::FsSandboxDenied
    );
    let writable = Policy::new(Mode::WorkspaceWrite, root.path()).unwrap();
    assert_eq!(
        writable.check_write(&target).unwrap(),
        canonical_target(&target).unwrap()
    );
    assert!(!target.exists());
    let system = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("mona-sandbox-denial.txt")
    } else {
        PathBuf::from("/etc/mona-sandbox-denial.txt")
    };
    assert_eq!(
        writable.check_write(&system).unwrap_err().code,
        ErrorCode::FsSandboxDenied
    );
    assert!(Policy::new(Mode::DangerFullAccess, root.path())
        .unwrap()
        .check_write(&system)
        .is_ok());
    assert!(Policy::new(Mode::WorkspaceWrite, "relative").is_err());
    assert!(writable.with_session("x".repeat(129)).is_err());
}
#[test]
fn profiles_preserve_dsh_file_only_vocabulary_and_exact_argv() {
    let dir = tempfile::tempdir().unwrap();
    let readonly = Policy::new(Mode::ReadOnly, dir.path()).unwrap();
    let args = profiles::bubblewrap(&readonly);
    assert_eq!(
        args,
        [
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--die-with-parent"
        ]
        .map(OsString::from)
    );
    let rw = Policy::new(Mode::WorkspaceWrite, dir.path()).unwrap();
    let args = profiles::bubblewrap(&rw);
    assert_eq!(args[args.len() - 2], rw.workspace_root.as_os_str());
    assert_eq!(args[args.len() - 1], rw.workspace_root.as_os_str());
    let profile = profiles::seatbelt(&rw).unwrap();
    assert!(profile.contains("(deny file-write*)") && profile.contains("(allow default)"));
    assert!(!profile.contains("deny network"));
    assert!(!profiles::seatbelt(&readonly).unwrap().contains("subpath"));
}
#[test]
fn diagnostics_are_bounded_and_runner_exit_gated() {
    let mut d = Diagnostics::new(Backend::WindowsAcl);
    d.push(b"mona-windows-acl-");
    d.push(b"run: cannot create token\n");
    assert_eq!(
        d.classify(Some(127), false),
        Some(Diagnostic::RunnerFailure)
    );
    let mut d = Diagnostics::new(Backend::WindowsAcl);
    d.push(b"mona-windows-acl-run: cleanup: failed\n");
    assert_eq!(
        d.classify(Some(127), false),
        Some(Diagnostic::CommandFailure)
    );
    let mut d = Diagnostics::new(Backend::Landlock);
    d.push(b"mona-landlock-run: partial enforcement (older Landlock ABI)\npermission denied\n");
    assert_eq!(d.classify(Some(125), false), Some(Diagnostic::Denied));
    let mut d = Diagnostics::new(Backend::Bubblewrap);
    d.push(&vec![b'x'; 1_000_000]);
    assert_eq!(d.classify(Some(0), true), None);
}
#[tokio::test]
async fn unrestricted_does_not_need_runner_and_cancelled_preparation_never_spawns() {
    let root = tempfile::tempdir().unwrap();
    let provider = LocalSandbox::new(Runner::standalone(root.path().join("missing-runner")));
    let policy = Policy::new(Mode::DangerFullAccess, root.path()).unwrap();
    let args = [
        OsString::from("payload"),
        OsString::from("literal argument"),
    ];
    let stop = CancellationToken::new();
    let prepared = provider.prepare(&args, &policy, &stop).await.unwrap();
    assert_eq!(prepared.program, "payload");
    assert_eq!(prepared.args, args[1..]);
    assert!(prepared.info.is_none());
    stop.cancel();
    assert!(
        matches!(provider.prepare(&args, &policy, &stop).await, Err(e) if e.code==ErrorCode::SandboxCancelled)
    );
    provider.shutdown().await.unwrap();
}
#[cfg(unix)]
#[test]
fn symlink_then_parent_uses_filesystem_order_not_lexical_shortcut() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b/deep");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    symlink(&b, a.join("link")).unwrap();
    assert_eq!(
        canonical_target(&a.join("link/../new.txt")).unwrap(),
        std::fs::canonicalize(dir.path().join("b"))
            .unwrap()
            .join("new.txt")
    );
    symlink(dir.path().join("missing"), a.join("dangling")).unwrap();
    assert!(canonical_target(&a.join("dangling/x")).is_err());
}
