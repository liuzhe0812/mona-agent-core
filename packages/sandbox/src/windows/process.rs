//! A small single-threaded launcher; parent Tools still own capture, deadline and cancellation.
use super::native::*;
use crate::{Error, Result};
use std::{
    ffi::OsString,
    mem::size_of,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Console::*, JobObjects::*, Threading::*},
    UI::WindowsAndMessaging::{ShowWindow, SW_HIDE},
};

fn command_line(argv: &[OsString]) -> Result<Vec<u16>> {
    let mut line = Vec::new();
    for (index, arg) in argv.iter().enumerate() {
        if index != 0 {
            line.push(32);
        }
        let mut chars = wide(arg.as_os_str())?;
        chars.pop();
        // Always quote, doubling backslashes only before quotes and at the closing quote.
        line.push(34);
        let mut slashes = 0;
        for c in chars {
            if c == 92 {
                slashes += 1;
                continue;
            }
            if c == 34 {
                line.extend(std::iter::repeat_n(92, slashes * 2 + 1));
            } else {
                line.extend(std::iter::repeat_n(92, slashes));
            }
            slashes = 0;
            line.push(c);
        }
        line.extend(std::iter::repeat_n(92, slashes * 2));
        line.push(34);
    }
    if line.len() >= 32767 {
        return Err(Error::config(
            "Windows sandbox command line exceeds 32767 UTF-16 units",
        ));
    }
    line.push(0);
    Ok(line)
}
struct Inherit {
    handles: Vec<(HANDLE, u32)>,
}
impl Inherit {
    fn enable(&mut self, handle: HANDLE) -> Result<()> {
        let mut flags = 0;
        // SAFETY: borrowed valid standard handle; original inheritance restored on all paths.
        boolean(
            unsafe { GetHandleInformation(handle, &mut flags) },
            "GetHandleInformation",
        )?;
        boolean(
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
            "SetHandleInformation(inherit)",
        )?;
        self.handles.push((handle, flags));
        Ok(())
    }
}
impl Drop for Inherit {
    fn drop(&mut self) {
        for &(handle, flags) in &self.handles {
            if unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flags & HANDLE_FLAG_INHERIT)
            } == 0
            {
                eprintln!(
                    "mona-windows-acl-run: cleanup: standard handle inheritance restore failed"
                );
            }
        }
    }
}
struct Child {
    process: Handle,
    _thread: Handle,
    armed: bool,
}
impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: an uncommitted spawn is killed even if Job assignment or resume fails.
        if self.armed {
            unsafe {
                TerminateProcess(self.process.0, 127);
            }
        }
    }
}

pub(super) fn run(token: Handle, argv: &[OsString], cwd: &Path) -> Result<i32> {
    if argv.is_empty() {
        return Err(Error::config("missing native payload"));
    }
    let mut line = command_line(argv)?;
    // Native canonical identity may use the verbatim prefix; ordinary child shells
    // expect drive/UNC spelling for their current directory, as in DSH's realpath.
    let spelling = cwd.to_string_lossy();
    let cwd = if let Some(unc) = spelling.strip_prefix("\\\\?\\UNC\\") {
        wide(std::ffi::OsStr::new(&format!("\\\\{unc}")))?
    } else {
        wide(std::ffi::OsStr::new(
            spelling.strip_prefix("\\\\?\\").unwrap_or(&spelling),
        ))?
    };
    let selectors = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    let mut stdio = [null_mut(); 3];
    for (index, selector) in selectors.into_iter().enumerate() {
        let value = unsafe { GetStdHandle(selector) };
        if value.is_null() || value == INVALID_HANDLE_VALUE {
            return Err(Error::unavailable(
                "native sandbox requires valid inherited standard handles",
            ));
        }
        stdio[index] = value;
    }
    // DSH's restricted child must inherit a console, not use CREATE_NO_WINDOW or
    // CREATE_NEW_CONSOLE. GUI hosts may have none; the TRUSTED launcher establishes
    // a hidden console first and restores captured stdio. The payload stays restricted.
    let mut process_id = 0;
    if unsafe { GetConsoleProcessList(&mut process_id, 1) } == 0 {
        boolean(unsafe { AllocConsole() }, "AllocConsole")?;
        let window = unsafe { GetConsoleWindow() };
        if !window.is_null() {
            unsafe {
                ShowWindow(window, SW_HIDE);
            }
        }
        for (selector, handle) in selectors.into_iter().zip(stdio) {
            boolean(unsafe { SetStdHandle(selector, handle) }, "SetStdHandle")?;
        }
    }
    // Set encoding in the trusted launcher: read-only PowerShell deliberately enters
    // ConstrainedLanguage and cannot instantiate UTF8Encoding (also documented by DSH).
    boolean(unsafe { SetConsoleCP(65001) }, "SetConsoleCP(UTF-8)")?;
    boolean(
        unsafe { SetConsoleOutputCP(65001) },
        "SetConsoleOutputCP(UTF-8)",
    )?;
    let mut inherit = Inherit {
        handles: Vec::new(),
    };
    for handle in stdio {
        inherit.enable(handle)?;
    }
    let job = Handle::new(
        unsafe { CreateJobObjectW(null(), null()) },
        "CreateJobObjectW",
    )?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    boolean(
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        },
        "SetInformationJobObject",
    )?;
    let start = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW,
        wShowWindow: 0,
        hStdInput: stdio[0],
        hStdOutput: stdio[1],
        hStdError: stdio[2],
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all C strings are terminated, structures correctly sized, token owned,
    // inherited handles live. Suspended child cannot execute before Job assignment.
    boolean(
        unsafe {
            CreateProcessAsUserW(
                token.0,
                null(),
                line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED,
                null(),
                cwd.as_ptr(),
                &start,
                &mut info,
            )
        },
        "CreateProcessAsUserW",
    )?;
    let mut child = Child {
        process: Handle::new(info.hProcess, "CreateProcessAsUserW(process)")?,
        _thread: Handle::new(info.hThread, "CreateProcessAsUserW(thread)")?,
        armed: true,
    };
    boolean(
        unsafe { AssignProcessToJobObject(job.0, child.process.0) },
        "AssignProcessToJobObject",
    )?;
    drop(inherit);
    if unsafe { ResumeThread(child._thread.0) } == u32::MAX {
        boolean(0, "ResumeThread")?;
    }
    let waited = unsafe { WaitForSingleObject(child.process.0, INFINITE) };
    if waited != WAIT_OBJECT_0 {
        return Err(Error::unavailable(format!(
            "WaitForSingleObject returned {waited}"
        )));
    }
    let mut code = 0;
    boolean(
        unsafe { GetExitCodeProcess(child.process.0, &mut code) },
        "GetExitCodeProcess",
    )?;
    child.armed = false;
    // Close before returning to terminate every descendant retaining output handles.
    drop(job);
    Ok(code as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_argv_quoting_preserves_backslashes_quotes_and_empty_arguments() {
        let line = command_line(&[
            OsString::from("C:\\Program Files\\app.exe"),
            OsString::from(""),
            OsString::from("x\\\"y"),
            OsString::from("end\\"),
        ])
        .unwrap();
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "\"C:\\Program Files\\app.exe\" \"\" \"x\\\\\\\"y\" \"end\\\\\""
        );
        assert!(command_line(&[OsString::from("bad\0arg")]).is_err());
    }
}
