//! The Windows side of [`super::cli::check_cli_file`]: what Unix checks
//! with owner and mode bits, read from the file's owner and access list.
//!
//! The CLI is handed the private key and its passphrase, so it must be a
//! program only the user or the system can change. Owned by the user,
//! LocalSystem, Administrators or TrustedInstaller, and no access entry
//! lets anyone else write to it, delete it or change its permissions. That
//! leaves Program Files and the user's own profile (winget, Scoop, a copy
//! in Downloads) and refuses a folder like `C:\tools`, where Windows lets
//! every signed-in user change what is created under `C:\`.

use std::path::Path;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    GetAce, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_CALLBACK_ACE, ACE_HEADER, ACL,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, ACCESS_DENIED_CALLBACK_ACE_TYPE,
    ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_DENIED_OBJECT_ACE_TYPE,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const LOCAL_SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
/// OWNER RIGHTS: stands for the owner, which is checked on its own.
const OWNER_RIGHTS: &str = "S-1-3-4";

const INHERIT_ONLY_ACE: u8 = 0x08;

/// Rights that let someone replace what runs: write or append (add a file
/// or folder, for a folder), delete it or a child, change its permissions
/// or owner, and the generic rights that include those.
const REPLACING_RIGHTS: u32 = 0x0002 // FILE_WRITE_DATA / FILE_ADD_FILE
    | 0x0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
    | 0x0040 // FILE_DELETE_CHILD
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE

/// Owned by the user or the system, and nobody else can replace it.
/// `name` is the manager's, at the start of every message.
pub fn check_not_replaceable(name: &str, path: &Path, what: &str) -> Result<(), String> {
    let me = current_user_sid().map_err(|e| format!("{name}: cannot read who runs Reacher CM: {e}"))?;
    let (owner, writers) = read_access(path)
        .map_err(|e| format!("{name}: the permissions of {what} at {} cannot be read: {e}", path.display()))?;
    let trusted = |sid: &str| sid == me || [LOCAL_SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER].contains(&sid);
    if !trusted(&owner) {
        return Err(format!("{name}: {what} at {} belongs to another user", path.display()));
    }
    if writers.iter().any(|sid| !trusted(sid) && sid != OWNER_RIGHTS) {
        return Err(format!("{name}: {what} at {} can be changed by other users", path.display()));
    }
    Ok(())
}

/// The owner, and every account an entry that applies to this object lets
/// replace it. `None` access list (no DACL) means everyone may.
fn read_access(path: &Path) -> Result<(String, Vec<String>), String> {
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the out pointers point into the security descriptor the call
    // allocates, which is freed below after the last read of them.
    let status = unsafe {
        GetNamedSecurityInfoW(
            &HSTRING::from(path.as_os_str()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        )
    };
    if status.is_err() {
        return Err(format!("{status:?}"));
    }
    let result = (|| {
        let owner = sid_string(owner)?;
        if dacl.is_null() {
            return Ok((owner, vec!["S-1-1-0".to_string()]));
        }
        // SAFETY: `dacl` points to a valid ACL inside the live descriptor.
        Ok((owner, unsafe { possible_writers(dacl) }?))
    })();
    // SAFETY: `sd` was allocated by GetNamedSecurityInfoW for the caller to free.
    unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    result
}

/// `dacl` must point to a valid Windows ACL for this call.
unsafe fn possible_writers(dacl: *const ACL) -> Result<Vec<String>, String> {
    let mut writers = Vec::new();
    let count = unsafe { (*dacl).AceCount };
    for i in 0..u32::from(count) {
        let mut ace = std::ptr::null_mut();
        unsafe { GetAce(dacl, i, &mut ace) }.map_err(|e| e.to_string())?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if header.AceFlags & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        // Treat a callback allow as a possible grant without trying to
        // evaluate its condition against another user's security token.
        let (mask, sid) = match u32::from(header.AceType) {
            ACCESS_ALLOWED_ACE_TYPE => {
                let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                (allowed.Mask, std::ptr::addr_of!(allowed.SidStart))
            }
            ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_CALLBACK_ACE>() };
                (allowed.Mask, std::ptr::addr_of!(allowed.SidStart))
            }
            ACCESS_DENIED_ACE_TYPE | ACCESS_DENIED_OBJECT_ACE_TYPE | ACCESS_DENIED_CALLBACK_ACE_TYPE
            | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE => continue,
            other => return Err(format!("unsupported access control entry type {other}")),
        };
        if mask & REPLACING_RIGHTS != 0 {
            writers.push(sid_string(PSID(sid.cast_mut().cast()))?);
        }
    }
    Ok(writers)
}

fn sid_string(sid: PSID) -> Result<String, String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is a valid SID; the string is allocated for us to free.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|e| e.to_string())?;
    let s = unsafe { text.to_string() }.map_err(|e| e.to_string());
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    s
}

fn current_user_sid() -> Result<String, String> {
    // SAFETY: this process's own token, closed on every path; the buffer is
    // sized from the API's own answer and outlives the read of the SID in it.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| e.to_string())?;
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        let read = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &mut needed);
        let _ = CloseHandle(token);
        read.map_err(|e| e.to_string())?;
        sid_string((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::BOOL;
    use windows::Win32::Security::{Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityDescriptorDacl};

    #[test]
    fn conditional_write_grants_count_as_possible_writers() {
        // Everyone can write conditionally. The authenticated-users grant
        // only inherits to children and must not count for this object.
        let sddl = HSTRING::from(r#"D:(XA;;FW;;;WD;(@User.Title=="PM"))(XA;OICIIO;FW;;;AU;(@User.Title=="PM"))"#);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, 1, &mut sd, None) }.unwrap();
        let mut dacl = std::ptr::null_mut();
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let writers = unsafe {
            GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    assert!(present.as_bool() && !dacl.is_null());
                    possible_writers(dacl)
                })
        };
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        assert_eq!(writers.unwrap(), ["S-1-1-0"]);
    }
}
