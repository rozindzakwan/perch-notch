//! Mail passwords in Windows Credential Manager (generic credentials named
//! `Perch/<key>`, persisted on this machine only, never roaming). Visible and
//! removable in Control Panel > Credential Manager > Windows Credentials.

use perch_core::error::{Error, Result};
use perch_core::platform::SecretStore;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// [`SecretStore`] over Credential Manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsCredentials;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn target(key: &str) -> Vec<u16> {
    wide(&format!("Perch/{key}"))
}

fn not_found(e: &windows::core::Error) -> bool {
    e.code() == ERROR_NOT_FOUND.to_hresult()
}

impl SecretStore for WindowsCredentials {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let t = target(key);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `t` is a NUL-terminated UTF-16 string that outlives the call;
        // on success `cred` points to a buffer we free with CredFree below.
        if let Err(e) = unsafe { CredReadW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0, &mut cred) } {
            if not_found(&e) {
                return Ok(None);
            }
            return Err(Error::backend(format!("Credential Manager read: {e}")));
        }
        if cred.is_null() {
            return Ok(None);
        }
        // SAFETY: CredReadW succeeded, so `cred` is a valid CREDENTIALW whose
        // blob pointer/size describe readable memory until CredFree.
        let secret = unsafe {
            let c = &*cred;
            let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
            };
            CredFree(cred as *const _);
            bytes
        };
        String::from_utf8(secret)
            .map(Some)
            .map_err(|_| Error::backend("stored secret is not UTF-8"))
    }

    fn set(&self, key: &str, label: &str, secret: &str) -> Result<()> {
        let mut t = target(key);
        let mut comment = wide(label);
        let mut user = wide(key.split_once(':').map(|(_, r)| r).unwrap_or(key));
        let mut blob = secret.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(t.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: u32::try_from(blob.len())
                .map_err(|_| Error::backend("secret too long"))?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: every pointer in `cred` refers to a live local buffer for the
        // duration of the call; CredWriteW copies what it keeps.
        let r = unsafe { CredWriteW(&cred, 0) };
        blob.fill(0);
        r.map_err(|e| Error::backend(format!("Credential Manager write: {e}")))
    }

    fn delete(&self, key: &str) -> Result<()> {
        let t = target(key);
        // SAFETY: NUL-terminated UTF-16 target name that outlives the call.
        match unsafe { CredDeleteW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0) } {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(Error::backend(format!("Credential Manager delete: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes, reads and deletes a throwaway `Perch/test:...` credential.
    #[test]
    fn round_trip_in_credential_manager() {
        let s = WindowsCredentials;
        let key = format!("test:roundtrip-{}@localhost", std::process::id());
        assert_eq!(s.get(&key).ok().flatten(), None);
        s.set(&key, "Perch test credential", "pä55 wörd \"x\"")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            s.get(&key).ok().flatten().as_deref(),
            Some("pä55 wörd \"x\"")
        );
        s.set(&key, "Perch test credential", "second")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(s.get(&key).ok().flatten().as_deref(), Some("second"));
        s.delete(&key).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(s.get(&key).ok().flatten(), None);
        s.delete(&key)
            .unwrap_or_else(|e| panic!("deleting twice is fine: {e}"));
    }
}
