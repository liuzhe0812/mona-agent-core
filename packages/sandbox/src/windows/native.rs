//! Audited allocation/handle boundary. No raw native pointer escapes this module family.
use crate::{Error, Result};
use std::{
    ffi::{c_void, OsStr},
    mem::{offset_of, size_of},
    os::windows::ffi::OsStrExt,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, Security::Authorization::*, Security::*};

pub(super) fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(Error::config("native argument contains NUL"));
    }
    value.push(0);
    Ok(value)
}
pub(super) fn status(code: u32, api: &str) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(Error::unavailable(format!(
            "{api}: Win32 {code}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )))
    }
}
pub(super) fn boolean(result: i32, api: &str) -> Result<()> {
    if result != 0 {
        Ok(())
    } else {
        // SAFETY: thread-local last error is captured before any cleanup call.
        status(unsafe { GetLastError() }.max(1), api)
    }
}
pub(super) struct Handle(pub HANDLE);
impl Handle {
    pub fn new(handle: HANDLE, api: &str) -> Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            boolean(0, api)?;
        }
        Ok(Self(handle))
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owns one non-null non-pseudo kernel handle, never cloned.
        if unsafe { CloseHandle(self.0) } == 0 {
            eprintln!("mona-windows-acl-run: cleanup: CloseHandle failed");
        }
    }
}
pub(super) struct Allocation(pub *mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: only OS allocations documented as LocalFree-owned enter this type.
            if !unsafe { LocalFree(self.0) }.is_null() {
                eprintln!("mona-windows-acl-run: cleanup: LocalFree failed");
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Sid(Vec<u32>);
impl Sid {
    pub fn parse(value: &str) -> Result<Self> {
        let text = wide(OsStr::new(value))?;
        let mut raw: PSID = null_mut();
        // SAFETY: NUL-terminated input; valid out pointer; converted allocation is owned below.
        boolean(
            unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut raw) },
            "ConvertStringSidToSidW",
        )?;
        let allocation = Allocation(raw);
        // SAFETY: successful conversion supplies a complete SID until allocation is dropped.
        unsafe { Self::copy(allocation.0) }
    }
    pub unsafe fn copy(raw: PSID) -> Result<Self> {
        // SAFETY: caller supplies an OS-owned, live SID. The result copies all bytes.
        boolean(unsafe { IsValidSid(raw) }, "IsValidSid")?;
        let length = unsafe { GetLengthSid(raw) };
        if !(8..=68).contains(&length) {
            return Err(Error::unavailable("invalid native SID length"));
        }
        let mut data = vec![0u32; (length as usize).div_ceil(4)];
        boolean(
            unsafe { CopySid(length, data.as_mut_ptr().cast(), raw) },
            "CopySid",
        )?;
        Ok(Self(data))
    }
    pub fn ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
    pub fn len(&self) -> usize {
        self.0.len() * 4
    }
}

pub(super) fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>> {
    let mut size = 0;
    // SAFETY: size query carries no destination buffer. The second call uses its bounded size.
    unsafe {
        GetTokenInformation(token, class, null_mut(), 0, &mut size);
    }
    if size == 0 || size > 1024 * 1024 {
        return Err(Error::unavailable(
            "GetTokenInformation returned invalid size",
        ));
    }
    let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    boolean(
        unsafe { GetTokenInformation(token, class, data.as_mut_ptr().cast(), size, &mut size) },
        "GetTokenInformation",
    )?;
    Ok(data)
}
pub(super) fn logon_sid(token: HANDLE) -> Result<Sid> {
    let data = token_info(token, TokenGroups)?;
    let bytes = data.len() * size_of::<usize>();
    if bytes < offset_of!(TOKEN_GROUPS, Groups) {
        return Err(Error::unavailable("TokenGroups truncated"));
    }
    // SAFETY: aligned initialized buffer returned by TokenGroups; array count checked before reads.
    let count = unsafe { *(data.as_ptr().cast::<u32>()) } as usize;
    let offset = offset_of!(TOKEN_GROUPS, Groups);
    if count > (bytes - offset) / size_of::<SID_AND_ATTRIBUTES>() {
        return Err(Error::unavailable("TokenGroups count out of range"));
    }
    for i in 0..count {
        let group = unsafe {
            &*(data
                .as_ptr()
                .cast::<u8>()
                .add(offset + i * size_of::<SID_AND_ATTRIBUTES>())
                .cast::<SID_AND_ATTRIBUTES>())
        };
        if group.Attributes & 0xc000_0000 == 0xc000_0000 {
            return unsafe { Sid::copy(group.Sid) };
        }
    }
    Err(Error::unavailable("current token has no logon SID"))
}

pub(super) fn entry(sid: &Sid, mode: ACCESS_MODE, mask: u32, flags: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: flags,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.ptr().cast(),
        },
    }
}
pub(super) fn merge(old: *const ACL, entries: &[EXPLICIT_ACCESS_W]) -> Result<Allocation> {
    let mut new_acl = null_mut();
    // SAFETY: old ACL remains live in its security descriptor; each entry SID remains live.
    status(
        unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), old, &mut new_acl) },
        "SetEntriesInAclW",
    )?;
    if new_acl.is_null() {
        return Err(Error::unavailable("SetEntriesInAclW returned null ACL"));
    }
    Ok(Allocation(new_acl.cast()))
}

pub(super) fn restricted_token(mode: crate::Mode, writes: &[Sid]) -> Result<Handle> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut raw = null_mut();
    // SAFETY: obtains an owned token from the current process pseudo handle.
    boolean(
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
                &mut raw,
            )
        },
        "OpenProcessToken",
    )?;
    let current = Handle::new(raw, "OpenProcessToken")?;
    let logon = logon_sid(current.0)?;
    let world = Sid::parse("S-1-1-0")?;
    let low = Sid::parse("S-1-16-4096")?;
    let mut sids = vec![
        SID_AND_ATTRIBUTES {
            Sid: logon.ptr(),
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: world.ptr(),
            Attributes: 0,
        },
    ];
    if mode == crate::Mode::WorkspaceWrite {
        if writes.is_empty() {
            return Err(Error::config("workspace-write requires capability SIDs"));
        }
        sids.extend(writes.iter().map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid.ptr(),
            Attributes: 0,
        }));
    }
    let mut token = null_mut();
    // SAFETY: SID arrays and current token live across the call; no disabled SID/privilege arrays.
    boolean(
        unsafe {
            CreateRestrictedToken(
                current.0,
                DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
                0,
                null(),
                0,
                null(),
                sids.len() as u32,
                sids.as_ptr(),
                &mut token,
            )
        },
        "CreateRestrictedToken",
    )?;
    let token = Handle::new(token, "CreateRestrictedToken")?;
    let size = size_of::<TOKEN_MANDATORY_LABEL>() + low.len();
    let mut info = vec![0usize; size.div_ceil(size_of::<usize>())];
    // SAFETY: aligned buffer has room for the label and required trailing extent; SID is live.
    unsafe {
        info.as_mut_ptr()
            .cast::<TOKEN_MANDATORY_LABEL>()
            .write(TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: low.ptr(),
                    Attributes: 0x20, /* SE_GROUP_INTEGRITY */
                },
            });
    }
    boolean(
        unsafe {
            SetTokenInformation(
                token.0,
                TokenIntegrityLevel,
                info.as_ptr().cast(),
                size as u32,
            )
        },
        "SetTokenInformation(Low integrity)",
    )?;
    let dacl = token_info(token.0, TokenDefaultDacl)?;
    if dacl.len() * size_of::<usize>() < size_of::<TOKEN_DEFAULT_DACL>() {
        return Err(Error::unavailable("token default DACL truncated"));
    }
    let old = unsafe { &*dacl.as_ptr().cast::<TOKEN_DEFAULT_DACL>() };
    if old.DefaultDacl.is_null() {
        return Err(Error::unavailable("restricted token has no default DACL"));
    }
    // Default-DACL grant lets grandchildren create pipes/sync objects; their parent folder
    // still gates file creation. Prefer the private-temp SID, as DSH does.
    let grant = writes.last().unwrap_or(&world);
    let merged = merge(
        old.DefaultDacl,
        &[entry(grant, GRANT_ACCESS, 0x001f_01ff, 0)],
    )?;
    let adjusted = TOKEN_DEFAULT_DACL {
        DefaultDacl: merged.0.cast(),
    };
    boolean(
        unsafe {
            SetTokenInformation(
                token.0,
                TokenDefaultDacl,
                (&adjusted as *const TOKEN_DEFAULT_DACL).cast(),
                size_of::<TOKEN_DEFAULT_DACL>() as u32,
            )
        },
        "SetTokenInformation(default DACL)",
    )?;
    Ok(token)
}
