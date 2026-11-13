//! Run-at-login via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Per-user only: Perch never needs admin rights.

use perch_core::error::{Error, Result};
use perch_core::platform::Autostart;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("Perch");

/// Registry-backed autostart toggle.
#[derive(Debug, Default)]
pub struct RegistryAutostart;

struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a key we successfully opened and have not closed
        // yet; this is the matching close.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<OwnedKey> {
    let mut key = HKEY::default();
    // SAFETY: RUN_KEY is a static wide string literal, `key` is a valid local
    // out-param, and we close the key in OwnedKey::drop.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, access, &mut key) };
    if status.is_err() {
        return Err(Error::unavailable(
            "autostart",
            format!("cannot open HKCU Run key: {status:?}"),
        ));
    }
    Ok(OwnedKey(key))
}

/// Quoted path of the current executable, as the Run key expects.
fn command_line() -> Result<HSTRING> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::io("determining the Perch executable path", e))?;
    Ok(HSTRING::from(format!("\"{}\"", exe.display())))
}

impl Autostart for RegistryAutostart {
    fn is_enabled(&self) -> Result<bool> {
        let key = open(KEY_READ)?;
        let mut size = 0u32;
        // SAFETY: all pointers are either null (we only want the size) or valid
        // locals; `key.0` is open for reading.
        let status =
            unsafe { RegQueryValueExW(key.0, VALUE_NAME, None, None, None, Some(&mut size)) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        if status.is_err() {
            return Err(Error::unavailable(
                "autostart",
                format!("cannot read the Run value: {status:?}"),
            ));
        }
        Ok(size > 0)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        let key = open(KEY_WRITE)?;
        if enabled {
            let cmd = command_line()?;
            let bytes: &[u8] = {
                let wide = cmd.as_wide();
                // SAFETY: reinterpreting a &[u16] as &[u8] of twice the length
                // is always valid for reading: u16 has no padding or niche, and
                // the lifetime is tied to `wide`, which outlives this slice.
                unsafe {
                    std::slice::from_raw_parts(
                        wide.as_ptr() as *const u8,
                        std::mem::size_of_val(wide) + 2, // include the NUL
                    )
                }
            };
            // SAFETY: `key.0` is open for writing, VALUE_NAME is a static wide
            // literal, and `bytes` is a valid read-only slice for its length.
            let status = unsafe { RegSetValueExW(key.0, VALUE_NAME, 0, REG_SZ, Some(bytes)) };
            if status.is_err() {
                return Err(Error::unavailable(
                    "autostart",
                    format!("cannot write the Run value: {status:?}"),
                ));
            }
        } else {
            // SAFETY: `key.0` is open for writing and VALUE_NAME is a static
            // wide literal.
            let status = unsafe { RegDeleteValueW(key.0, VALUE_NAME) };
            if status.is_err() && status != ERROR_FILE_NOT_FOUND {
                return Err(Error::unavailable(
                    "autostart",
                    format!("cannot delete the Run value: {status:?}"),
                ));
            }
        }
        Ok(())
    }
}
