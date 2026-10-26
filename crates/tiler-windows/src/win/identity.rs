//! Who we are: the current user's SID, session, and a pipe ACL for them.

use std::ffi::c_void;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};

/// The current user's SID as a string, e.g. `S-1-5-21-…`.
pub fn user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle for our own process; valid out pointer.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let mut buf = vec![0u8; 256];
    let mut len = 0u32;
    // SAFETY: buffer of the size we pass; TOKEN_USER plus its SID fit in 256 bytes.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            buf.len() as u32,
            &mut len,
        )
    };
    // SAFETY: we own the token handle.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
    ok.ok()?;
    // SAFETY: GetTokenInformation filled the buffer with a TOKEN_USER.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let mut text = PWSTR::null();
    // SAFETY: the SID points into `buf`, which is still alive.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.ok()?;
    // SAFETY: Windows allocated a NUL-terminated string that we free with LocalFree.
    let sid = unsafe { text.to_string() }.ok();
    unsafe { LocalFree(Some(HLOCAL(text.0 as *mut c_void))) };
    sid
}

/// The Windows session we run in (console and remote desktop sessions differ).
pub fn session_id() -> u32 {
    let mut session = 0;
    // SAFETY: valid out pointer.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    session
}

/// Security attributes that give only `sid` access. Must be kept alive while used.
pub struct OwnerOnly {
    descriptor: PSECURITY_DESCRIPTOR,
    pub attributes: SECURITY_ATTRIBUTES,
}

impl OwnerOnly {
    pub fn new(sid: &str) -> Option<OwnerOnly> {
        // Owner and a protected DACL with one entry: full access for this user.
        let sddl = HSTRING::from(format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: valid SDDL string and out pointer.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .ok()?;
        Some(OwnerOnly {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            },
        })
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by Windows with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
    }
}
