//! Small Windows services: toast notifications, opening files, idle time and
//! the local UTC offset.

use std::path::Path;
use std::time::Duration;

use perch_core::error::{Error, Result};
use perch_core::platform::{IdleSource, Notifier, Opener};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::{SetCurrentProcessExplicitAppUserModelID, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

/// The AppUserModelID toasts are attributed to.
pub const AUMID: &str = "Perch.Notch";

/// Register Perch's AppUserModelID for the current user and adopt it for this
/// process, which is what unpackaged desktop apps need for toasts
/// (`HKCU\Software\Classes\AppUserModelId\<id>`, no admin, no shortcut).
pub fn register_app_id(icon: Option<&Path>) -> Result<()> {
    let key_path = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
    let mut key = HKEY::default();
    // SAFETY: creating/opening a per-user key with valid out-param.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &key_path,
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if status.is_err() {
        return Err(Error::backend(format!(
            "RegCreateKeyExW(AppUserModelId): {status:?}"
        )));
    }
    set_string(key, w!("DisplayName"), "Perch");
    if let Some(icon) = icon {
        set_string(key, w!("IconUri"), &icon.to_string_lossy());
    }
    // SAFETY: closing the key we opened.
    let _ = unsafe { RegCloseKey(key) };
    // SAFETY: static wide string.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID)) }
        .map_err(|e| Error::backend(format!("SetCurrentProcessExplicitAppUserModelID: {e}")))
}

fn set_string(key: HKEY, name: PCWSTR, value: &str) {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` outlives the call; byte view of a u16 slice is valid.
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    // SAFETY: `key` is open for writing.
    let _ = unsafe { RegSetValueExW(key, name, 0, REG_SZ, Some(bytes)) };
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Native toast notifications.
#[derive(Debug, Default)]
pub struct ToastNotifier;

impl Notifier for ToastNotifier {
    fn notify(&mut self, summary: &str, body: &str) -> Result<()> {
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual><audio silent=\"true\"/></toast>",
            xml_escape(summary),
            xml_escape(body)
        );
        let doc =
            XmlDocument::new().map_err(|e| Error::unavailable("notifications", e.to_string()))?;
        doc.LoadXml(&HSTRING::from(xml))
            .map_err(|e| Error::unavailable("notifications", e.to_string()))?;
        let toast = ToastNotification::CreateToastNotification(&doc)
            .map_err(|e| Error::unavailable("notifications", e.to_string()))?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
            .map_err(|e| Error::unavailable("notifications", e.to_string()))?;
        notifier
            .Show(&toast)
            .map_err(|e| Error::unavailable("notifications", e.to_string()))
    }
}

/// `ShellExecuteW("open")`.
#[derive(Debug, Default)]
pub struct ShellOpener;

impl Opener for ShellOpener {
    fn open(&self, path: &Path) -> Result<()> {
        let target = HSTRING::from(path.as_os_str());
        // SAFETY: all strings are valid for the call; no window owner.
        let r = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &target,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute returns a value > 32 on success.
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(Error::backend(format!(
                "ShellExecuteW failed ({})",
                r.0 as isize
            )))
        }
    }
}

/// Idle time from `GetLastInputInfo`.
#[derive(Debug, Default)]
pub struct LastInputIdle;

impl IdleSource for LastInputIdle {
    fn idle_for(&self) -> Result<Duration> {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a correctly sized out-param.
        if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
            return Err(Error::backend("GetLastInputInfo failed"));
        }
        // SAFETY: no arguments. Both values are 32-bit tick counts, so the
        // wrapping subtraction is right even across the 49.7-day rollover.
        let now = unsafe { GetTickCount() };
        Ok(Duration::from_millis(u64::from(
            now.wrapping_sub(info.dwTime),
        )))
    }
}

/// Local UTC offset in seconds (local = UTC + offset), DST-aware.
pub fn utc_offset_seconds() -> i32 {
    let mut tz = TIME_ZONE_INFORMATION::default();
    // SAFETY: valid out-param.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    // 2 = TIME_ZONE_ID_DAYLIGHT
    let bias = tz.Bias
        + if id == 2 {
            tz.DaylightBias
        } else {
            tz.StandardBias
        };
    -bias * 60
}

/// Attach to the parent process's console (a terminal that launched us), so a
/// GUI-subsystem binary can still print `--help` and CLI errors. A no-op when
/// launched from Explorer.
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: no pointers; failure (no parent console) is expected and ignored.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// Installed physical memory in bytes (0 if unknown).
pub fn total_ram_bytes() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `m` is a correctly sized out-param.
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() {
        m.ullTotalPhys
    } else {
        0
    }
}

/// Press Ctrl+V in the focused window (used after dictation).
pub fn send_paste() -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL,
    };
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let v = VIRTUAL_KEY(u16::from(b'V'));
    let inputs = [
        key(VK_CONTROL, false),
        key(v, false),
        key(v, true),
        key(VK_CONTROL, true),
    ];
    // SAFETY: a valid array of fully initialised keyboard inputs.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(Error::backend(
            "SendInput was blocked (e.g. by an elevated window)",
        ))
    }
}
