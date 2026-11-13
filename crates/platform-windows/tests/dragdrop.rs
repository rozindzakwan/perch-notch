//! The shelf's drag source builds a shell data object that offers CF_HDROP,
//! which is what Explorer, mail clients and browsers accept as a file drop.
//! (The modal `DoDragDrop` loop itself needs real mouse input, so it is not
//! run here.)
#![cfg(windows)]

use perch_platform_windows::dragdrop::build_data_object;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};

#[test]
fn shelf_files_become_a_cf_hdrop_data_object() {
    // SAFETY: initialising OLE (STA) for this test thread, as winit does for
    // the UI thread; balanced by OleUninitialize below.
    unsafe { windows::Win32::System::Ole::OleInitialize(None) }.unwrap_or_else(|e| panic!("{e}"));
    let dir = std::env::temp_dir().join("perch-drag-test");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let file = dir.join("shelf item.txt");
    std::fs::write(&file, b"perch").unwrap_or_else(|e| panic!("{e}"));

    let data = build_data_object(std::slice::from_ref(&file)).unwrap_or_else(|e| panic!("{e}"));
    let fmt = FORMATETC {
        cfFormat: 15, // CF_HDROP
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: querying a live data object with a valid FORMATETC.
    let hr = unsafe { data.QueryGetData(&fmt) };
    assert!(hr.is_ok(), "CF_HDROP offered: {hr:?}");

    assert!(
        build_data_object(&[dir.join("does-not-exist.bin")]).is_err(),
        "unresolvable paths are reported, not dragged"
    );
    drop(data);
    let _ = std::fs::remove_file(file);
    // SAFETY: balances OleInitialize above on the same thread.
    unsafe { windows::Win32::System::Ole::OleUninitialize() };
}
