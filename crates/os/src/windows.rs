#![allow(unsafe_code, reason = "Win32 calls without a safe wrapper")]

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::process::Child;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS,
    GetExitCodeProcess, OpenProcess, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};

const STILL_ACTIVE: u32 = 259;

pub(crate) fn process_alive(pid: u32) -> bool {
    // SAFETY: OpenProcess only reads its arguments and returns null on failure.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // SAFETY: reads the calling thread's last error code.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut code = 0;
    // SAFETY: handle is open and code is a valid out pointer.
    let queried = unsafe { GetExitCodeProcess(handle, &raw mut code) } != 0;
    // SAFETY: handle came from OpenProcess and is closed exactly once.
    unsafe { CloseHandle(handle) };
    queried && code == STILL_ACTIVE
}

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// `CreateProcessW` without handle inheritance, which `std::process` cannot do.
pub(crate) fn spawn_detached(program: &Path, args: &[OsString]) -> io::Result<u32> {
    let mut line = command_line(program, args);
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    match create_process(&mut line, base | CREATE_BREAKAWAY_FROM_JOB) {
        // the caller's job may forbid breakaway, detaching without it is still better
        Err(error) if error.raw_os_error() == i32::try_from(ERROR_ACCESS_DENIED).ok() => {
            create_process(&mut command_line(program, args), base)
        }
        other => other,
    }
}

fn create_process(line: &mut [u16], flags: u32) -> io::Result<u32> {
    // SAFETY: an all-zero startup structure is valid; null std handles mean no stdio.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = u32::try_from(size_of::<STARTUPINFOW>())
        .map_err(|_| io::Error::other("startup structure too large"))?;
    startup.dwFlags = STARTF_USESTDHANDLES;
    // SAFETY: an all-zero process information structure is a valid out value.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: line is a writable NUL-terminated buffer and the structures outlive the call.
    let created = unsafe {
        CreateProcessW(
            ptr::null(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            ptr::null(),
            ptr::null(),
            &raw const startup,
            &raw mut info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both handles came from CreateProcessW and are closed exactly once.
    unsafe {
        CloseHandle(info.hThread);
        CloseHandle(info.hProcess);
    }
    Ok(info.dwProcessId)
}

/// Quotes like the Microsoft C runtime parses, so the child sees the exact arguments.
fn command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let mut line = vec![QUOTE];
    line.extend(program.as_os_str().encode_wide());
    line.push(QUOTE);
    for arg in args {
        line.push(u16::from(b' '));
        push_argument(&mut line, arg);
    }
    line.push(0);
    line
}

fn push_argument(line: &mut Vec<u16>, arg: &OsStr) {
    let wide: Vec<u16> = arg.encode_wide().collect();
    let plain = !wide.is_empty()
        && !wide
            .iter()
            .any(|&unit| matches!(unit, 0x20 | 0x09 | 0x0a | 0x0b | QUOTE));
    if plain {
        line.extend(wide);
        return;
    }
    line.push(QUOTE);
    let mut backslashes = 0;
    for unit in wide {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                line.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
    line.push(QUOTE);
}

/// Job object that kills its processes when terminated or closed.
#[derive(Debug)]
pub(crate) struct Job {
    // the raw handle as an integer keeps the type Send without an unsafe impl
    handle: usize,
}

impl Job {
    pub(crate) fn assign(child: &Child) -> io::Result<Self> {
        // SAFETY: null attributes and name are allowed and create an unnamed job.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self {
            handle: handle as usize,
        };
        // SAFETY: an all-zero limit structure is a valid starting value.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(|_| io::Error::other("job limit structure too large"))?;
        // SAFETY: the pointer and size describe the limits structure above.
        let configured = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                size,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both handles are open for the duration of the call.
        let assigned = unsafe { AssignProcessToJobObject(job.raw(), child.as_raw_handle()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(crate) fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job handle stays open until drop.
        if unsafe { TerminateJobObject(self.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn raw(&self) -> HANDLE {
        self.handle as HANDLE
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed exactly once.
        unsafe { CloseHandle(self.raw()) };
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    use super::*;

    fn quoted(arg: &str) -> String {
        let mut line = Vec::new();
        push_argument(&mut line, OsStr::new(arg));
        String::from_utf16(&line).unwrap()
    }

    #[test]
    fn quotes_arguments_like_the_c_runtime_parses_them() {
        assert_eq!(quoted("plain"), "plain");
        assert_eq!(quoted(""), r#""""#);
        assert_eq!(quoted("two words"), r#""two words""#);
        assert_eq!(quoted(r"C:\dir\"), r"C:\dir\");
        assert_eq!(quoted(r"a b\"), r#""a b\\""#);
        assert_eq!(quoted(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quoted(r#"back\"quote"#), r#""back\\\"quote""#);
    }

    #[test]
    fn detached_processes_inherit_no_handles() {
        let (mut reader, writer) = io::pipe().unwrap();
        // SAFETY: the handle is open and owned by writer; a launcher may leave such pipes.
        let marked = unsafe {
            SetHandleInformation(
                writer.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        };
        assert_ne!(marked, 0);
        let args = ["/C", "ping -n 5 127.0.0.1 > NUL"].map(OsString::from);
        spawn_detached(Path::new("cmd"), &args).unwrap();
        drop(writer);
        let started = Instant::now();
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the pipe stayed open"
        );
    }
}
