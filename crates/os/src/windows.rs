#![allow(unsafe_code, reason = "Win32 calls without a safe wrapper")]

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NOT_FOUND, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree,
};
#[cfg(test)]
use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW, SetSecurityInfo,
};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
#[cfg(test)]
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, GetAce, GetSecurityDescriptorControl, SE_DACL_PROTECTED,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    CreateProcessW, DETACHED_PROCESS, GetCurrentProcess, GetExitCodeProcess, OpenProcess,
    OpenProcessToken, OpenThread, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, THREAD_SUSPEND_RESUME,
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
pub(crate) fn spawn_detached(program: &Path, args: &[OsString], cwd: &Path) -> io::Result<u32> {
    let cwd: Vec<u16> = cwd.as_os_str().encode_wide().chain(iter::once(0)).collect();
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let detached = create_process(
        &mut command_line(program, args),
        &cwd,
        base | CREATE_BREAKAWAY_FROM_JOB,
    );
    match detached {
        // the caller's job may forbid breakaway, detaching without it is still better
        Err(error) if error.raw_os_error() == i32::try_from(ERROR_ACCESS_DENIED).ok() => {
            create_process(&mut command_line(program, args), &cwd, base)
        }
        other => other,
    }
}

fn create_process(line: &mut [u16], cwd: &[u16], flags: u32) -> io::Result<u32> {
    // SAFETY: an all-zero startup structure is valid; null std handles mean no stdio.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = u32::try_from(size_of::<STARTUPINFOW>())
        .map_err(|_| io::Error::other("startup structure too large"))?;
    startup.dwFlags = STARTF_USESTDHANDLES;
    // SAFETY: an all-zero process information structure is a valid out value.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: line and cwd are NUL-terminated, line is writable, the structures outlive the call.
    let created = unsafe {
        CreateProcessW(
            ptr::null(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            ptr::null(),
            cwd.as_ptr(),
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
    handle: OwnedHandle,
}

impl Job {
    /// Spawns `command` suspended and resumes it only inside the job, so nothing escapes.
    pub(crate) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let job = Self::create()?;
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        if let Err(error) = job.assign(&child).and_then(|()| resume(child.id())) {
            // the suspended child never ran, so killing it loses nothing
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, job))
    }

    fn create() -> io::Result<Self> {
        // SAFETY: null attributes and name are allowed and create an unnamed job.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a fresh job handle that nothing else owns.
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
        };
        // SAFETY: an all-zero limit structure is a valid starting value.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(|_| io::Error::other("job limit structure too large"))?;
        // SAFETY: the pointer and size describe the limits structure above.
        let configured = unsafe {
            SetInformationJobObject(
                job.handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                size,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are open for the duration of the call.
        let assigned =
            unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), child.as_raw_handle()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job handle stays open until drop.
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Resumes every thread of a process that was created suspended.
fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: a thread snapshot takes no pointers.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is a fresh snapshot handle that nothing else owns.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: an all-zero entry is valid once its size is set.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>())
        .map_err(|_| io::Error::other("thread entry too large"))?;
    let mut resumed = 0;
    // SAFETY: the snapshot is open and entry is a valid out pointer.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: OpenThread only reads its arguments and returns null on failure.
            let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: raw is a fresh thread handle that nothing else owns.
            let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
            // SAFETY: the thread handle is open with resume rights.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed += 1;
        }
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(format!("process {pid} has no thread")));
    }
    Ok(())
}

/// Largest secret Windows keeps in one credential.
pub(crate) const CREDENTIAL_MAX: usize = 2560;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(iter::once(0)).collect()
}

/// The generic credential stored under `target`, if any.
pub(crate) fn credential_read(target: &str) -> io::Result<Option<Vec<u8>>> {
    let target = wide(target);
    let mut found: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: target is NUL-terminated and found is a valid out pointer.
    let read = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &raw mut found) };
    if read == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == i32::try_from(ERROR_NOT_FOUND).ok() {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: CredReadW filled found with a credential whose blob holds the stated size.
    let blob = unsafe {
        let credential = &*found;
        let size = usize::try_from(credential.CredentialBlobSize).unwrap_or(0);
        if size == 0 || credential.CredentialBlob.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(credential.CredentialBlob, size).to_vec()
        }
    };
    // SAFETY: found came from CredReadW and is freed exactly once.
    unsafe { CredFree(found.cast::<c_void>()) };
    Ok(Some(blob))
}

/// Stores or replaces the generic credential under `target`, kept for this machine.
pub(crate) fn credential_write(target: &str, user: &str, secret: &[u8]) -> io::Result<()> {
    let size = u32::try_from(secret.len())
        .ok()
        .filter(|_| secret.len() <= CREDENTIAL_MAX)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "secret too large"))?;
    let mut target = wide(target);
    let mut user = wide(user);
    let mut blob = secret.to_vec();
    // SAFETY: an all-zero credential is valid; the fields set below are all that is used.
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target.as_mut_ptr();
    credential.UserName = user.as_mut_ptr();
    credential.CredentialBlobSize = size;
    credential.CredentialBlob = blob.as_mut_ptr();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    // SAFETY: the credential points at buffers that outlive the call; Windows copies them.
    let written = unsafe { CredWriteW(&raw const credential, 0) };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the generic credential under `target`; a missing one is no error.
pub(crate) fn credential_delete(target: &str) -> io::Result<()> {
    let target = wide(target);
    // SAFETY: target is NUL-terminated.
    let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if deleted == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != i32::try_from(ERROR_NOT_FOUND).ok() {
            return Err(error);
        }
    }
    Ok(())
}

fn path_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}

/// The SID as text, such as `S-1-5-21-...`.
fn sid_text(sid: PSID) -> io::Result<String> {
    let mut raw: *mut u16 = ptr::null_mut();
    // SAFETY: the caller passes a valid SID and raw is a valid out pointer.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    // SAFETY: raw is NUL-terminated, so the scan stops inside the allocation.
    while unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the first `length` units were just read.
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, length) });
    // SAFETY: raw was allocated by the conversion and is freed once.
    unsafe { LocalFree(raw.cast::<c_void>()) };
    Ok(text)
}

/// The user of this process: the SID in the token, which stays the same when run elevated.
pub(crate) struct UserSid {
    // u64 units keep the TOKEN_USER structure aligned
    buffer: Vec<u64>,
    text: String,
}

impl UserSid {
    pub(crate) fn current() -> io::Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: the pseudo handle of this process is always valid and token is an out pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: token was just opened and is closed exactly once, whatever the outcome.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0_u32;
        // SAFETY: a null buffer of size 0 only asks for the size.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &raw mut needed,
            );
        }
        let units = usize::try_from(needed)
            .map_err(|_| io::Error::other("token information too large"))?
            .div_ceil(8)
            .max(1);
        let mut buffer = vec![0_u64; units];
        // SAFETY: buffer holds at least `needed` bytes and token is open with query rights.
        let read = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast::<c_void>(),
                needed,
                &raw mut needed,
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut sid = Self {
            buffer,
            text: String::new(),
        };
        sid.text = sid_text(sid.pointer())?;
        Ok(sid)
    }

    fn pointer(&self) -> PSID {
        // SAFETY: the buffer was filled by GetTokenInformation(TokenUser) and is aligned for it.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    /// The SID as text, such as `S-1-5-21-...`.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// Makes the current user the owner and the only one with access to the file or folder.
pub(crate) fn restrict_to_current_user(path: &Path) -> io::Result<()> {
    let target = path_wide(path);
    restrict_with(|owner, dacl| {
        // SAFETY: target is NUL-terminated; owner and dacl outlive the call.
        unsafe {
            SetNamedSecurityInfoW(
                target.as_ptr().cast_mut(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                owner,
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        }
    })
}

/// Access rights a handle needs for [`restrict_handle_to_current_user`].
pub(crate) const RESTRICT_ACCESS: u32 = 0x4000_0000 | 0x0004_0000 | 0x0008_0000;

/// Like [`restrict_to_current_user`] through an open handle, so no sharing is needed.
pub(crate) fn restrict_handle_to_current_user(file: &File) -> io::Result<()> {
    restrict_with(|owner, dacl| {
        // SAFETY: the handle has write-DAC and write-owner rights; owner and dacl outlive the call.
        unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                owner,
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        }
    })
}

/// Builds the owner-only DACL and hands the owner and the list to `apply`.
fn restrict_with(apply: impl FnOnce(PSID, *const ACL) -> u32) -> io::Result<()> {
    let user = UserSid::current()?;
    let sddl = wide(&format!("D:P(A;OICI;FA;;;{})", user.text()));
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: sddl is NUL-terminated and descriptor is a valid out pointer.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: descriptor came from the conversion above and the out pointers are valid.
    let found = unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &raw mut present,
            &raw mut dacl,
            &raw mut defaulted,
        )
    };
    let result = if found == 0 || present == 0 {
        Err(io::Error::last_os_error())
    } else {
        let status = apply(user.pointer(), dacl);
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(
                i32::try_from(status).unwrap_or(i32::MAX),
            ))
        }
    };
    // SAFETY: descriptor was allocated by the conversion and is freed exactly once.
    unsafe { LocalFree(descriptor) };
    result
}

/// Owner, protection and allow entries (SID text and access mask) of a file or folder.
#[cfg(test)]
pub(crate) struct AclSummary {
    pub(crate) owner: String,
    pub(crate) protected: bool,
    pub(crate) entries: u16,
    pub(crate) allowed: Vec<(String, u32)>,
}

#[cfg(test)]
pub(crate) fn acl_summary(path: &Path) -> io::Result<AclSummary> {
    let target = path_wide(path);
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: target is NUL-terminated and the out pointers are valid.
    let status = unsafe {
        GetNamedSecurityInfoW(
            target.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ));
    }
    let summary = read_summary(descriptor, owner, dacl);
    // SAFETY: descriptor was allocated by the call above and is freed once.
    unsafe { LocalFree(descriptor) };
    summary
}

#[cfg(test)]
fn read_summary(
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
) -> io::Result<AclSummary> {
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor is valid and the out pointers are valid.
    if unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if dacl.is_null() {
        return Err(io::Error::other("no access list"));
    }
    // SAFETY: dacl points into the descriptor, which is still allocated.
    let entries = unsafe { (*dacl).AceCount };
    let mut allowed = Vec::new();
    for index in 0..u32::from(entries) {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: index is below the entry count and ace is a valid out pointer.
        if unsafe { GetAce(dacl, index, &raw mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: GetAce returned an entry inside the list; type 0 is an allow entry.
        if unsafe { (*ace).Header.AceType } == 0 {
            // SAFETY: an allow entry stores its SID from SidStart on.
            let sid = unsafe { (&raw mut (*ace).SidStart).cast::<c_void>() };
            // SAFETY: as above.
            let mask = unsafe { (*ace).Mask };
            allowed.push((sid_text(sid)?, mask));
        }
    }
    Ok(AclSummary {
        owner: sid_text(owner)?,
        protected: control & SE_DACL_PROTECTED != 0,
        entries,
        allowed,
    })
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
        spawn_detached(Path::new("cmd"), &args, &std::env::temp_dir()).unwrap();
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
