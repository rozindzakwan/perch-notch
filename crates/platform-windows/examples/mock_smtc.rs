//! A minimal SMTC "player" for end-to-end tests.
//!
//! Creates a genuine System Media Transport Controls session through the
//! documented desktop-app API (`ISystemMediaTransportControlsInterop::
//! GetForWindow`), publishes a known title/artist, and toggles play/pause when
//! the OS forwards button presses (which is how Perch's commands arrive). Any
//! SMTC client (Perch, the Windows volume flyout) sees it as a real player.
//!
//! Usage: `mock_smtc [TITLE] [ARTIST] [--cover IMAGE]`; runs until its window is
//! closed or the process is killed. `--cover` publishes the image as a
//! file-backed thumbnail (StorageFile), like many real players do.

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::core::{w, HSTRING};
    use windows::Foundation::TypedEventHandler;
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    };
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::{
        ISystemMediaTransportControlsInterop, RoInitialize, RO_INIT_SINGLETHREADED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, HMENU, MSG, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
    };

    unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: default handling with the arguments we were given.
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    let mut cover = None;
    let mut rest = Vec::new();
    let mut all = std::env::args().skip(1);
    while let Some(a) = all.next() {
        if a == "--cover" {
            cover = all.next();
        } else {
            rest.push(a);
        }
    }
    let mut args = rest.into_iter();
    let title = args.next().unwrap_or_else(|| "Mock Song".into());
    let artist = args.next().unwrap_or_else(|| "Mock Artist".into());

    // SAFETY: initialising WinRT for this (single) thread before any WinRT use.
    unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
    // SAFETY: GetModuleHandleW(None) returns this executable's module.
    let hinstance = unsafe { GetModuleHandleW(None)? };
    let class = w!("PerchMockSmtc");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: hinstance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: a fully initialised class description.
    unsafe { RegisterClassW(&wc) };
    // SAFETY: class registered above; a hidden top-level window owning the session.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("Perch mock SMTC player"),
            WS_OVERLAPPED,
            0,
            0,
            100,
            100,
            None,
            HMENU::default(),
            hinstance,
            None,
        )?
    };

    let interop: ISystemMediaTransportControlsInterop = windows::core::factory::<
        SystemMediaTransportControls,
        ISystemMediaTransportControlsInterop,
    >()?;
    // SAFETY: `hwnd` is a live window created on this thread.
    let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd)? };
    smtc.SetIsEnabled(true)?;
    smtc.SetIsPlayEnabled(true)?;
    smtc.SetIsPauseEnabled(true)?;
    smtc.SetIsNextEnabled(true)?;
    smtc.SetIsPreviousEnabled(true)?;
    let du = smtc.DisplayUpdater()?;
    du.SetType(MediaPlaybackType::Music)?;
    let props = du.MusicProperties()?;
    props.SetTitle(&HSTRING::from(title))?;
    props.SetArtist(&HSTRING::from(artist))?;
    if let Some(path) = cover {
        let file =
            windows::Storage::StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.get()?;
        du.SetThumbnail(
            &windows::Storage::Streams::RandomAccessStreamReference::CreateFromFile(&file)?,
        )?;
    }
    du.Update()?;
    smtc.SetPlaybackStatus(MediaPlaybackStatus::Playing)?;

    smtc.ButtonPressed(&TypedEventHandler::new(
        move |s: &Option<SystemMediaTransportControls>,
              a: &Option<SystemMediaTransportControlsButtonPressedEventArgs>| {
            if let (Some(s), Some(a)) = (s, a) {
                let status = match a.Button()? {
                    SystemMediaTransportControlsButton::Play => MediaPlaybackStatus::Playing,
                    SystemMediaTransportControlsButton::Pause => MediaPlaybackStatus::Paused,
                    _ => return Ok(()),
                };
                s.SetPlaybackStatus(status)?;
                println!("button -> {status:?}");
            }
            Ok(())
        },
    ))?;
    println!("mock SMTC session up");

    let mut msg = MSG::default();
    // SAFETY: standard message loop for this thread's window.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("mock_smtc is Windows-only");
}
