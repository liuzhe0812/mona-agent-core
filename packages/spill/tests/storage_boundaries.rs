use api::ErrorCode;
use spill::{LocalSpillStore, SpillConfig, SpillStore};
use std::{fs, path::Path, sync::Arc};
use tempfile::tempdir;

fn config() -> SpillConfig {
    let mut config = SpillConfig::default();
    config.max_page_bytes = 8;
    config
}

#[tokio::test]
async fn utf8_pages_are_bounded_and_require_character_boundaries() {
    let root = tempdir().unwrap();
    let mut limits = config();
    limits.max_page_bytes = 4;
    let store = LocalSpillStore::new(root.path(), limits).unwrap();
    let record = store.put("run-a", "call", "a中béz").await.unwrap();

    let page = store.read_page("run-a", &record.id, 0, 4).await.unwrap();
    assert_eq!(page.text, "a中");
    assert_eq!(page.text.len(), 4);
    assert_eq!(page.next_offset, 4);

    let error = store
        .read_page("run-a", &record.id, 1, 2)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Limit);

    let error = store
        .read_page("run-a", &record.id, 2, 4)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Schema);
}

#[tokio::test]
async fn a_page_limit_smaller_than_the_first_character_is_an_error() {
    let root = tempdir().unwrap();
    let mut limits = config();
    limits.max_page_bytes = 1;
    let store = LocalSpillStore::new(root.path(), limits).unwrap();
    let record = store.put("run-a", "call", "中").await.unwrap();

    let error = store
        .read_page("run-a", &record.id, 0, 1)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Limit);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_storage_permissions_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    let store = LocalSpillStore::new(root.path(), config()).unwrap();
    let record = store.put("run-a", "call", "secret").await.unwrap();
    let run_dir = root.path().join("72756e2d61");
    let entry = run_dir.join(format!("{}.txt", record.id));

    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&run_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(entry).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn symlinked_storage_components_are_rejected() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let link = root.path().join("root-link");
    if !make_directory_link(outside.path(), &link) {
        eprintln!("skipping root symlink check: symlink creation is unavailable");
        return;
    }

    let store = LocalSpillStore::new(&link, config()).unwrap();
    assert!(store.put("run-a", "call", "secret").await.is_err());

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let run_link = root.path().join("72756e2d61");
    if !make_directory_link(outside.path(), &run_link) {
        eprintln!("skipping run symlink check: symlink creation is unavailable");
        return;
    }
    let store = LocalSpillStore::new(root.path(), config()).unwrap();
    assert!(store.put("run-a", "call", "secret").await.is_err());
}

#[tokio::test]
async fn entry_symlinks_are_rejected_before_reading() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let store = Arc::new(LocalSpillStore::new(root.path(), config()).unwrap());
    let record = store.put("run-a", "call", "secret").await.unwrap();
    let entry = root
        .path()
        .join("72756e2d61")
        .join(format!("{}.txt", record.id));
    let target = outside.path().join("outside.txt");
    fs::write(&target, "outside").unwrap();
    fs::remove_file(&entry).unwrap();
    if !make_file_link(&target, &entry) {
        eprintln!("skipping entry symlink check: symlink creation is unavailable");
        return;
    }

    assert!(store.read_page("run-a", &record.id, 0, 8).await.is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_storage_acl_is_private_on_root_run_and_entry() {
    use std::{os::windows::process::CommandExt, process::Command};

    let root = tempdir().unwrap();
    let store = LocalSpillStore::new(root.path(), config()).unwrap();
    let record = store.put("run-a", "call", "secret").await.unwrap();
    let run_dir = root.path().join("72756e2d61");
    let entry = run_dir.join(format!("{}.txt", record.id));
    let weaken_script = r#"
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1') -Force -ErrorAction Stop
$acl = Get-Acl -LiteralPath $env:MONA_SPILL_ACL_ENTRY
$rule = New-Object System.Security.AccessControl.FileSystemAccessRule('Everyone', [System.Security.AccessControl.FileSystemRights]::ReadAndExecute, [System.Security.AccessControl.InheritanceFlags]::None, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $env:MONA_SPILL_ACL_ENTRY -AclObject $acl
"#;
    let powershell = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let weakened = Command::new(&powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            weaken_script,
        ])
        .env("MONA_SPILL_ACL_ENTRY", entry.as_os_str())
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        weakened.status.success(),
        "failed to add weak ACL: {}",
        String::from_utf8_lossy(&weakened.stderr)
    );
    let reopened = LocalSpillStore::new(root.path(), config()).unwrap();
    reopened.read_page("run-a", &record.id, 0, 8).await.unwrap();
    let script = r#"
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1') -Force -ErrorAction Stop
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
foreach ($path in @($env:MONA_SPILL_ACL_ROOT, $env:MONA_SPILL_ACL_RUN, $env:MONA_SPILL_ACL_ENTRY)) {
    $acl = Get-Acl -LiteralPath $path
    $allows = @($acl.Access | Where-Object { $_.AccessControlType -eq 'Allow' })
    if ($allows.Count -eq 0) { throw 'missing allow rule' }
    foreach ($entry in $allows) {
        if ($entry.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid) {
            throw 'ACL grants another identity'
        }
    }
}
$rootAcl = Get-Acl -LiteralPath $env:MONA_SPILL_ACL_ROOT
if (-not $rootAcl.AreAccessRulesProtected) { throw 'root ACL is inherited' }
"#;
    let powershell = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let output = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("MONA_SPILL_ACL_ROOT", root.path().as_os_str())
        .env("MONA_SPILL_ACL_RUN", run_dir.as_os_str())
        .env("MONA_SPILL_ACL_ENTRY", entry.as_os_str())
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Get-Acl verification failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_directory_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
}

fn make_file_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
}
