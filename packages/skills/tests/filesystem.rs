use api::{CancellationToken, ErrorCode};
use skills::{Limits, LocalSkills, SkillProvider};
use std::path::Path;

fn document(name: &str, extra: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: >-\n  A multi-line\n  description.\n{extra}---\n{body}"
    )
}
fn write_bundle(root: &Path, directory: &str, name: &str, extra: &str, body: &str) {
    let path = root.join(directory);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("SKILL.md"), document(name, extra, body)).unwrap();
}
fn provider(roots: Vec<std::path::PathBuf>, limits: Limits) -> LocalSkills {
    LocalSkills::new("filesystem", roots, limits).unwrap()
}

#[tokio::test]
async fn shallow_catalog_precedence_and_fresh_body_loading() {
    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    write_bundle(project.path(), "a", "shared", "", "project body");
    write_bundle(project.path(), "b", "shared", "", "later body");
    write_bundle(user.path(), "shared", "shared", "", "user body");
    write_bundle(project.path(), ".hidden", "hidden", "", "must not appear");
    write_bundle(
        project.path(),
        "nested/child",
        "nested",
        "",
        "must not appear",
    );
    let fs = provider(
        vec![project.path().into(), user.path().into()],
        Limits {
            max_skills: 1,
            ..Default::default()
        },
    );
    let catalog = fs.list(CancellationToken::new()).await.unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].description, "A multi-line description.");
    assert_eq!(
        fs.load("shared", CancellationToken::new())
            .await
            .unwrap()
            .unwrap()
            .content,
        "project body"
    );
    std::fs::write(
        project.path().join("a/SKILL.md"),
        document("shared", "", "edited body"),
    )
    .unwrap();
    assert_eq!(
        fs.load("shared", CancellationToken::new())
            .await
            .unwrap()
            .unwrap()
            .content,
        "edited body"
    );
}

#[tokio::test]
async fn invocation_policy_reloads_and_invalid_policies_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    write_bundle(
        root.path(),
        "one",
        "one",
        "disable-model-invocation: yes\nuser-invocable: 'OFF'\n",
        "private body",
    );
    let fs = provider(vec![root.path().into()], Limits::default());
    let skill = fs
        .load("one", CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert!(!skill.summary.model_invocable && !skill.summary.user_invocable);
    assert_eq!(
        fs.read_resource("one", "file.txt", CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Policy
    );
    for flags in [
        "disableModelInvocation: true\n",
        "disable-model-invocation: maybe\n",
        "user-invocable: null\n",
    ] {
        std::fs::write(
            root.path().join("one/SKILL.md"),
            document("one", flags, "body"),
        )
        .unwrap();
        assert_eq!(
            fs.list(CancellationToken::new()).await.unwrap_err().code,
            ErrorCode::Schema
        );
    }
    std::fs::write(
        root.path().join("one/SKILL.md"),
        document("renamed", "user-invocable: 0\n", "body"),
    )
    .unwrap();
    assert!(fs
        .load("one", CancellationToken::new())
        .await
        .unwrap()
        .is_none());
    let renamed = fs
        .load("renamed", CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert!(renamed.summary.model_invocable && !renamed.summary.user_invocable);
}

#[tokio::test]
async fn resources_cannot_escape_bundles_or_return_binary_content() {
    let root = tempfile::tempdir().unwrap();
    write_bundle(root.path(), "one", "one", "", "body");
    std::fs::create_dir(root.path().join("one/references")).unwrap();
    std::fs::write(root.path().join("one/references/notes.txt"), "notes").unwrap();
    std::fs::write(root.path().join("secret.txt"), "outside bundle").unwrap();
    std::fs::write(root.path().join("one/binary.bin"), [0u8, 1, 2]).unwrap();
    std::fs::write(root.path().join("one/nonutf8.bin"), [255u8]).unwrap();
    let fs = provider(vec![root.path().into()], Limits::default());
    assert_eq!(
        fs.read_resource("one", "references/notes.txt", CancellationToken::new())
            .await
            .unwrap(),
        "notes"
    );
    for path in [
        "../secret.txt",
        "..\\secret.txt",
        "/secret.txt",
        "C:\\secret.txt",
        "notes.txt:stream",
        "references",
    ] {
        assert!(
            fs.read_resource("one", path, CancellationToken::new())
                .await
                .is_err(),
            "accepted {path}"
        );
    }
    for path in ["binary.bin", "nonutf8.bin"] {
        assert_eq!(
            fs.read_resource("one", path, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Schema
        );
    }
    std::fs::write(root.path().join("flat.md"), document("flat", "", "body")).unwrap();
    assert_eq!(
        fs.read_resource("flat", "one/SKILL.md", CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
}

#[tokio::test]
async fn missing_roots_are_empty_but_io_and_content_limits_are_errors() {
    let root = tempfile::tempdir().unwrap();
    let missing = provider(vec![root.path().join("missing")], Limits::default());
    assert!(missing
        .list(CancellationToken::new())
        .await
        .unwrap()
        .is_empty());
    std::fs::write(root.path().join("file"), "file").unwrap();
    let bad_root = provider(vec![root.path().join("file/child")], Limits::default());
    assert!(bad_root.list(CancellationToken::new()).await.is_err());
    write_bundle(root.path(), "one", "one", "", &"x".repeat(100));
    let limited = provider(
        vec![root.path().into()],
        Limits {
            max_content_bytes: 32,
            ..Default::default()
        },
    );
    assert_eq!(
        limited
            .load("one", CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Limit
    );
    let limited = provider(
        vec![root.path().into()],
        Limits {
            max_file_bytes: 16,
            ..Default::default()
        },
    );
    assert_eq!(
        limited
            .list(CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Limit
    );
    let limited = provider(
        vec![root.path().into()],
        Limits {
            max_entries_per_root: 1,
            ..Default::default()
        },
    );
    assert_eq!(
        limited
            .list(CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Limit
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        missing.list(cancelled).await.unwrap_err().code,
        ErrorCode::Cancelled
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected_for_instructions_and_resources() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_bundle(root.path(), "one", "one", "", "body");
    std::fs::write(outside.path().join("private.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("one/escape")).unwrap();
    let fs = provider(vec![root.path().into()], Limits::default());
    assert!(fs
        .read_resource("one", "escape/private.txt", CancellationToken::new())
        .await
        .is_err());
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    assert!(fs.list(CancellationToken::new()).await.is_err());
}
