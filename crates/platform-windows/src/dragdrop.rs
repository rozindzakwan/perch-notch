//! Drag files out of the shelf (OLE drag source).
//!
//! The shell builds the data object for the files (`IShellItemArray` bound to
//! `BHID_DataObject`, which carries CF_HDROP, shell ID lists and everything
//! Explorer, mail clients and browsers expect); Perch only supplies a minimal
//! `IDropSource`. `DoDragDrop` runs its own modal loop on the UI thread until
//! the button is released or Escape is pressed. winit initialises OLE on that
//! thread already (it enables drag-and-drop by default).

use std::path::PathBuf;

use perch_core::error::{Error, Result};
use windows::core::{implement, HRESULT, HSTRING};
use windows::Win32::Foundation::{BOOL, S_OK};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};

const DRAGDROP_S_DROP: HRESULT = HRESULT(0x0004_0100);
const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x0004_0101);
const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = HRESULT(0x0004_0102);

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Build the shell's data object for `paths` (CF_HDROP, shell ID lists, ...).
pub fn build_data_object(paths: &[PathBuf]) -> Result<IDataObject> {
    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    for p in paths {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: valid wide string and out-param; the PIDL is freed below.
        let r =
            unsafe { SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None) };
        if r.is_ok() && !pidl.is_null() {
            pidls.push(pidl);
        }
    }
    if pidls.is_empty() {
        return Err(Error::backend("none of the shelf files could be resolved"));
    }
    // SAFETY: `pidls` holds valid absolute PIDLs for the duration of the call.
    let array: windows::core::Result<IShellItemArray> =
        unsafe { SHCreateShellItemArrayFromIDLists(&pidls) };
    let data: windows::core::Result<IDataObject> = array.and_then(|a| {
        // SAFETY: binding a live shell item array to its data object handler.
        unsafe { a.BindToHandler(None, &BHID_DataObject) }
    });
    for pidl in &pidls {
        // SAFETY: each PIDL was allocated by SHParseDisplayName with the COM allocator.
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(*pidl as *const _)) };
    }
    data.map_err(|e| Error::backend(format!("shell data object: {e}")))
}

/// Start dragging `paths`. Returns once the drop finished or was cancelled.
pub fn drag_files(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let data = build_data_object(paths)?;
    let source: IDropSource = DropSource.into();
    let mut effect = DROPEFFECT::default();
    // SAFETY: both COM objects are valid; DoDragDrop runs a modal loop on this
    // (OLE-initialised) thread and returns when the drag ends.
    let hr = unsafe {
        DoDragDrop(
            &data,
            &source,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        )
    };
    if hr.is_err() {
        return Err(Error::backend(format!("DoDragDrop: {hr:?}")));
    }
    Ok(())
}
