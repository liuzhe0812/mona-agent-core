//! DSH's current-DACL merge, capability grant, ambient-delete deny and Low label.
use super::native::*;
use crate::{check_cancel, CancellationToken, Error, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    mem::size_of,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{Security::Authorization::*, Security::*, Storage::FileSystem::*};
const GRANT_MASK: u32 = 0x0011_0156;
const INHERIT: u32 = 3;

struct Security {
    _descriptor: Allocation,
    dacl: *mut ACL,
    label: *mut ACL,
}
fn security(path: &Path) -> Result<Security> {
    let path = wide(path.as_os_str())?;
    let (mut dacl, mut label, mut descriptor) = (null_mut(), null_mut(), null_mut());
    // SAFETY: correctly typed output pointers; all nested ACLs borrow the returned descriptor.
    status(
        unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                &mut label,
                &mut descriptor,
            )
        },
        "GetNamedSecurityInfoW",
    )?;
    Ok(Security {
        _descriptor: Allocation(descriptor),
        dacl,
        label,
    })
}

/// Examines only OS-returned ACL allocations; ACE and inline SID lengths are checked.
fn entries(acl: *const ACL, mut visit: impl FnMut(u8, u8, u32, PSID) -> bool) -> Result<bool> {
    if acl.is_null() {
        return Ok(false);
    }
    // SAFETY: pointer is borrowed from a live descriptor or initialized ACL buffer.
    let header = unsafe { &*acl };
    if header.AclSize < size_of::<ACL>() as u16 {
        return Err(Error::unavailable("invalid ACL header"));
    }
    for index in 0..u32::from(header.AceCount) {
        let mut raw = null_mut();
        boolean(unsafe { GetAce(acl, index, &mut raw) }, "GetAce")?;
        let base = acl as usize;
        let start = raw as usize;
        if start < base || start.saturating_add(8) > base + usize::from(header.AclSize) {
            return Err(Error::unavailable("ACL entry out of range"));
        }
        let ace = unsafe { &*raw.cast::<ACE_HEADER>() };
        if ace.AceSize < 16 || start + usize::from(ace.AceSize) > base + usize::from(header.AclSize)
        {
            return Err(Error::unavailable("invalid ACL entry extent"));
        }
        // Only simple allow/deny/mandatory ACEs have this layout; object ACEs are not interpreted.
        if ![0, 1, 17].contains(&ace.AceType) {
            continue;
        }
        let sid = unsafe { raw.cast::<u8>().add(8).cast() };
        let sid_len = 8 + 4 * usize::from(unsafe { *raw.cast::<u8>().add(9) });
        if sid_len + 8 > usize::from(ace.AceSize) {
            return Err(Error::unavailable("ACL SID out of range"));
        }
        boolean(unsafe { IsValidSid(sid) }, "IsValidSid(ACL)")?;
        let mask = unsafe { raw.cast::<u8>().add(4).cast::<u32>().read_unaligned() };
        if visit(ace.AceType, ace.AceFlags, mask, sid) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn exact(acl: *const ACL, kind: u8, flags: u8, mask: u32, sid: &Sid) -> Result<bool> {
    entries(acl, |k, f, m, p| {
        k == kind && f == flags && m == mask && unsafe { EqualSid(p, sid.ptr()) } != 0
    })
}
fn lock(path: &Path, cancel: &CancellationToken) -> Result<std::fs::File> {
    let digest = Sha256::digest(path.to_string_lossy().to_lowercase().as_bytes());
    let name: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let root = std::env::temp_dir().join("mona-sandbox-acl-locks");
    std::fs::create_dir_all(&root).map_err(Error::unavailable)?;
    // No DELETE share: the lock file cannot be replaced under an existing owner.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(root.join(format!("{name}.lock")))
        .map_err(Error::unavailable)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        check_cancel(cancel)?;
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => {
                return Err(Error::unavailable(format!(
                    "ACL path lock unavailable: {e}"
                )))
            }
        }
    }
}
fn low_label(low: &Sid) -> Result<Vec<u32>> {
    let size = size_of::<ACL>() + 8 + low.len();
    let mut data = vec![0u32; size.div_ceil(4)];
    let acl = data.as_mut_ptr().cast();
    // SAFETY: aligned writable allocation fits ACL header, mandatory ACE and complete SID.
    boolean(
        unsafe { InitializeAcl(acl, size as u32, ACL_REVISION) },
        "InitializeAcl",
    )?;
    boolean(
        unsafe {
            AddMandatoryAce(
                acl,
                ACL_REVISION,
                INHERIT,
                1, /* SYSTEM_MANDATORY_LABEL_NO_WRITE_UP */
                low.ptr(),
            )
        },
        "AddMandatoryAce",
    )?;
    Ok(data)
}

pub(super) fn grant(path: &Path, sid: &Sid, cancel: &CancellationToken) -> Result<()> {
    let _lock = lock(path, cancel)?;
    check_cancel(cancel)?;
    let old = security(path)?;
    let low = Sid::parse("S-1-16-4096")?;
    let world = Sid::parse("S-1-1-0")?;
    if exact(old.dacl, 0, 3, GRANT_MASK, sid)?
        && exact(old.dacl, 1, 2, FILE_DELETE_CHILD, &world)?
        && exact(old.label, 17, 3, 1, &low)?
    {
        return Ok(());
    }
    let label = low_label(&low)?;
    let new_acl = merge(
        old.dacl,
        &[
            entry(
                &world,
                DENY_ACCESS,
                FILE_DELETE_CHILD,
                CONTAINER_INHERIT_ACE,
            ),
            entry(sid, GRANT_ACCESS, GRANT_MASK, INHERIT),
        ],
    )?;
    let name = wide(path.as_os_str())?;
    check_cancel(cancel)?;
    // SAFETY: borrowed SIDs/ACLs remain live. One apply preserves all unrelated current ACEs.
    status(
        unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                new_acl.0.cast(),
                label.as_ptr().cast(),
            )
        },
        "SetNamedSecurityInfoW(grant)",
    )
    .map_err(|error| grant_error(path, error))
}

fn grant_error(path: &Path, error: Error) -> Error {
    Error::unavailable(format!(
        "{}; sandbox grant target: {}. The host must have WRITE_DAC and WRITE_OWNER on this directory to apply the DACL and Low integrity label (Modify alone is insufficient). Use a workspace and temp root with those rights; no permission elevation or unconfined fallback was attempted.",
        error.message, path.display()
    ))
}
pub(super) fn revoke(path: &Path, sid: &Sid) -> Result<()> {
    let _lock = lock(path, &CancellationToken::new())?;
    let old = security(path)?;
    let present = entries(old.dacl, |kind, _, _, p| {
        kind == 0 && unsafe { EqualSid(p, sid.ptr()) } != 0
    })?;
    if !present {
        return Ok(());
    }
    let foreign = entries(old.dacl, |kind, _, mask, p| {
        kind == 0 && mask == GRANT_MASK && unsafe { EqualSid(p, sid.ptr()) } == 0
    })?;
    let new_acl = merge(old.dacl, &[entry(sid, REVOKE_ACCESS, 0, INHERIT)])?;
    let name = wide(path.as_os_str())?;
    let flags = DACL_SECURITY_INFORMATION
        | if foreign {
            0
        } else {
            LABEL_SECURITY_INFORMATION
        };
    // SAFETY: REVOKE_ACCESS removes only this capability; surviving grants retain their label.
    status(
        unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                flags,
                null_mut(),
                null_mut(),
                new_acl.0.cast(),
                null(),
            )
        },
        "SetNamedSecurityInfoW(revoke)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insufficient_label_rights_fail_closed_with_actionable_directory_error() {
        // Alter only this test-owned directory, never the repository or user root.
        let dir = tempfile::tempdir().unwrap();
        let world = Sid::parse("S-1-1-0").unwrap();
        let capability = Sid::parse("S-1-4-55555-66666").unwrap();
        let original = security(dir.path()).unwrap();
        let restricted =
            merge(original.dacl, &[entry(&world, DENY_ACCESS, WRITE_OWNER, 0)]).unwrap();
        let name = wide(dir.path().as_os_str()).unwrap();
        // SAFETY: both ACLs borrow live allocations; the test retains WRITE_DAC to restore them.
        status(
            unsafe {
                SetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    restricted.0.cast(),
                    null(),
                )
            },
            "test deny WRITE_OWNER",
        )
        .unwrap();
        let denied = grant(dir.path(), &capability, &CancellationToken::new());
        let observed = security(dir.path()).unwrap();
        let capability_added = exact(observed.dacl, 0, 3, GRANT_MASK, &capability).unwrap();
        status(
            unsafe {
                SetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    original.dacl,
                    null(),
                )
            },
            "test restore DACL",
        )
        .unwrap();
        let error = denied.unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::SandboxUnavailable);
        assert!(error.message.contains("WRITE_OWNER"));
        assert!(error.message.contains(&dir.path().display().to_string()));
        assert!(
            !capability_added,
            "failed label setup must not publish a usable grant"
        );
    }

    #[test]
    fn grant_is_exact_idempotent_and_revoke_preserves_other_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let a = Sid::parse("S-1-4-11111-22222").unwrap();
        let b = Sid::parse("S-1-4-33333-44444").unwrap();
        let c = CancellationToken::new();
        grant(dir.path(), &a, &c).unwrap();
        grant(dir.path(), &a, &c).unwrap();
        grant(dir.path(), &b, &c).unwrap();
        revoke(dir.path(), &a).unwrap();
        let security = security(dir.path()).unwrap();
        assert!(!exact(security.dacl, 0, 3, GRANT_MASK, &a).unwrap());
        assert!(exact(security.dacl, 0, 3, GRANT_MASK, &b).unwrap());
        assert!(exact(
            security.label,
            17,
            3,
            1,
            &Sid::parse("S-1-16-4096").unwrap()
        )
        .unwrap());
        drop(security);
        revoke(dir.path(), &b).unwrap();
    }
}
