#![allow(unsafe_code, reason = "Win32 calls without a safe wrapper")]

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
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
