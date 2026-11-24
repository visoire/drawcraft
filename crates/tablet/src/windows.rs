//! Windows `WM_POINTER*` hook extracting high-frequency pen pressure, tilt, rotation,
//! barrel button, and inverted eraser state alongside winit.

#![allow(unsafe_code, clippy::upper_case_acronyms)]

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::Sample;
use crate::win_ink::{
    PenState, RawPenPacket, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER,
    WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE, dib_to_bmp_bytes,
    is_pointer_promoted_mouse_extra_info, is_supported_image_extension,
    should_clear_stale_pen_on_mouse, trim_png_trailing_bytes,
};

const MAX_HISTORY: usize = 64;
const MAX_QUEUED_SAMPLES: usize = 2048;
const NO_PEN: u32 = u32::MAX;
const PM_REMOVE: usize = 1;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_KEYDOWN: u32 = 0x0100;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_POINTERDEVICECHANGE: u32 = 0x0238;
const WM_POINTERDEVICEINRANGE: u32 = 0x0239;

const POINTER_DEVICE_PRODUCT_STRING_MAX: usize = 520;
const POINTER_DEVICE_TYPE_INTEGRATED_PEN: u32 = 0x0000_0001;
const POINTER_DEVICE_TYPE_EXTERNAL_PEN: u32 = 0x0000_0002;
const SM_DIGITIZER: i32 = 94;
const NID_INTEGRATED_PEN: i32 = 0x0000_0004;
const NID_EXTERNAL_PEN: i32 = 0x0000_0008;
const NID_READY: i32 = 0x0000_0080;
const MAX_POINTER_DEVICES: usize = 64;

const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_INSERT: usize = 0x2D;
const VK_V: usize = 0x56;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const GMEM_MOVEABLE: u32 = 0x0002;
const MAX_CLIPBOARD_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CLIPBOARD_TEXT_BYTES: usize = 2 * 1024 * 1024;

static BARREL_DOWN: AtomicBool = AtomicBool::new(false);
static SECONDARY_BARREL_DOWN: AtomicBool = AtomicBool::new(false);
static BARREL_PRESS_COUNT: AtomicU32 = AtomicU32::new(0);
static SECONDARY_BARREL_PRESS_COUNT: AtomicU32 = AtomicU32::new(0);
static DEVICES_DIRTY: AtomicBool = AtomicBool::new(true);
static PASTE_REQUESTED: AtomicBool = AtomicBool::new(false);
static PEN_ID: AtomicU32 = AtomicU32::new(NO_PEN);
static PEN_END: AtomicU8 = AtomicU8::new(0);
static CONTACT_END: AtomicU8 = AtomicU8::new(0);
static LAST_PEN_TIME_MS: AtomicU32 = AtomicU32::new(0);
static HAS_PEN_TIME: AtomicBool = AtomicBool::new(false);

static SAMPLE_STATE: Mutex<SharedSamples> = Mutex::new(SharedSamples {
    latest: None,
    queue: VecDeque::new(),
});

struct SharedSamples {
    latest: Option<Sample>,
    queue: VecDeque<Sample>,
}

fn encode_end(end: Option<bool>) -> u8 {
    match end {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

fn decode_end(v: u8) -> Option<bool> {
    match v {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

#[repr(C)]
struct CWPSTRUCT {
    l_param: isize,
    w_param: usize,
    message: u32,
    hwnd: *mut std::ffi::c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MSG {
    hwnd: *mut std::ffi::c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: POINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct POINTER_INFO {
    pointer_type: u32,
    pointer_id: u32,
    frame_id: u32,
    pointer_flags: u32,
    source_device: *mut std::ffi::c_void,
    hwnd_target: *mut std::ffi::c_void,
    pt_pixel_location: POINT,
    pt_himetric_location: POINT,
    pt_pixel_location_raw: POINT,
    pt_himetric_location_raw: POINT,
    dw_time: u32,
    history_count: u32,
    input_data: i32,
    dw_key_states: u32,
    performance_count: u64,
    button_change_type: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct POINTER_PEN_INFO {
    pointer_info: POINTER_INFO,
    pen_flags: u32,
    pen_mask: u32,
    pressure: u32,
    rotation: u32,
    tilt_x: i32,
    tilt_y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct POINTER_DEVICE_INFO {
    display_orientation: u32,
    device: *mut std::ffi::c_void,
    pointer_device_type: u32,
    monitor: *mut std::ffi::c_void,
    starting_cursor_id: u32,
    max_active_contacts: u16,
    product_string: [u16; POINTER_DEVICE_PRODUCT_STRING_MAX],
}

#[repr(C)]
struct OPENFILENAMEW {
    l_struct_size: u32,
    hwnd_owner: *mut std::ffi::c_void,
    h_instance: *mut std::ffi::c_void,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: isize,
    lpfn_hook: *const std::ffi::c_void,
    lp_template_name: *const u16,
    pv_reserved: *mut std::ffi::c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowsHookExW(
        id_hook: i32,
        lpfn: unsafe extern "system" fn(i32, usize, isize) -> isize,
        h_mod: *mut std::ffi::c_void,
        dw_thread_id: u32,
    ) -> *mut std::ffi::c_void;
    fn UnhookWindowsHookEx(hhk: *mut std::ffi::c_void) -> i32;
    fn CallNextHookEx(
        hhk: *mut std::ffi::c_void,
        n_code: i32,
        w_param: usize,
        l_param: isize,
    ) -> isize;
    fn GetCurrentThreadId() -> u32;
    fn GetActiveWindow() -> *mut std::ffi::c_void;
    fn GetMessageExtraInfo() -> isize;
    fn GetMessageTime() -> i32;
    fn GetKeyState(n_virt_key: i32) -> i16;
    fn OpenClipboard(h_wnd_new_owner: *mut std::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetClipboardData(u_format: u32) -> *mut std::ffi::c_void;
    fn SetClipboardData(u_format: u32, h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn RegisterClipboardFormatW(lpsz_format: *const u16) -> u32;
    fn GetPointerPenInfo(pointer_id: u32, pen_info: *mut POINTER_PEN_INFO) -> i32;
    fn GetPointerPenInfoHistory(
        pointer_id: u32,
        entries_count: *mut u32,
        pen_info: *mut POINTER_PEN_INFO,
    ) -> i32;
    fn GetPointerDevices(device_count: *mut u32, pointer_devices: *mut POINTER_DEVICE_INFO) -> i32;
    fn GetSystemMetrics(n_index: i32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(u_flags: u32, dw_bytes: usize) -> *mut std::ffi::c_void;
    fn GlobalFree(h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalLock(h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GlobalUnlock(h_mem: *mut std::ffi::c_void) -> i32;
    fn GlobalSize(h_mem: *mut std::ffi::c_void) -> usize;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragQueryFileW(
        h_drop: *mut std::ffi::c_void,
        i_file: u32,
        lpsz_file: *mut u16,
        cch: u32,
    ) -> u32;
    fn SHCreateItemFromParsingName(
        psz_path: *const u16,
        pbc: *mut std::ffi::c_void,
        riid: *const ComGuid,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(pv_reserved: *const std::ffi::c_void, dw_co_init: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        rclsid: *const ComGuid,
        p_unk_outer: *mut std::ffi::c_void,
        dw_cls_context: u32,
        riid: *const ComGuid,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn CoTaskMemFree(pv: *const std::ffi::c_void);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ComGuid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl ComGuid {
    const fn from_u128(uuid: u128) -> Self {
        Self {
            data1: (uuid >> 96) as u32,
            data2: ((uuid >> 80) & 0xffff) as u16,
            data3: ((uuid >> 64) & 0xffff) as u16,
            data4: (uuid as u64).to_be_bytes(),
        }
    }
}

const CLSID_FILE_OPEN_DIALOG: ComGuid = ComGuid::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);
const IID_I_FILE_DIALOG: ComGuid = ComGuid::from_u128(0x42f85136_db7e_439c_85f1_e4075d135fc8);
const IID_I_SHELL_ITEM: ComGuid = ComGuid::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const RPC_E_CHANGED_MODE: i32 = -2_147_417_850; // 0x80010106
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const FOS_NOCHANGEDIR: u32 = 0x0000_0008;
const FOS_PICKFOLDERS: u32 = 0x0000_0020;
const FOS_FORCEFILESYSTEM: u32 = 0x0000_0040;
const SIGDN_FILESYSPATH: i32 = -2_147_123_200; // 0x80058000

#[cfg(test)]
const IUNKNOWN_VTBL_LEN: usize = 3;
#[cfg(test)]
const IFILE_DIALOG_VTBL_LEN: usize = 27;
#[cfg(test)]
const ISHELL_ITEM_VTBL_LEN: usize = 8;
const VTBL_IUNKNOWN_RELEASE: usize = 2;
const VTBL_IMODAL_WINDOW_SHOW: usize = 3;
const VTBL_IFILE_DIALOG_SET_OPTIONS: usize = 9;
const VTBL_IFILE_DIALOG_GET_OPTIONS: usize = 10;
const VTBL_IFILE_DIALOG_SET_FOLDER: usize = 12;
const VTBL_IFILE_DIALOG_SET_TITLE: usize = 17;
const VTBL_IFILE_DIALOG_GET_RESULT: usize = 20;
const VTBL_ISHELL_ITEM_GET_DISPLAY_NAME: usize = 5;

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
    fn GetSaveFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
}

const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
const OFN_HIDEREADONLY: u32 = 0x0000_0004;
const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
const OFN_EXPLORER: u32 = 0x0008_0000;

fn str_to_wide_null(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().filter(|&c| c != 0).collect();
    wide.push(0);
    wide
}

fn os_str_to_wide_null(s: &OsStr) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_wide().filter(|&c| c != 0).collect();
    wide.push(0);
    wide
}

fn build_multi_filter_wide(filter_label: &str, patterns: &[&str]) -> Vec<u16> {
    let joined = patterns.join(";");
    let desc = format!("{filter_label} ({joined})");
    let mut wide = Vec::new();
    wide.extend(desc.encode_utf16().filter(|&c| c != 0));
    wide.push(0);
    wide.extend(joined.encode_utf16().filter(|&c| c != 0));
    wide.push(0);
    wide.extend("All Files (*.*)".encode_utf16());
    wide.push(0);
    wide.extend("*.*".encode_utf16());
    wide.push(0);
    wide.push(0);
    wide
}

fn build_filter_wide(filter_label: &str, ext: &str) -> Vec<u16> {
    let clean_ext = ext.trim_start_matches('.');
    let pattern = format!("*.{clean_ext}");
    build_multi_filter_wide(filter_label, &[&pattern])
}

struct FileDialogSpec<'a> {
    save_mode: bool,
    title: &'a str,
    filter_wide: Vec<u16>,
    def_ext: &'a str,
    default_name: Option<&'a str>,
    initial_dir: Option<&'a Path>,
}

fn run_file_dialog(spec: FileDialogSpec<'_>) -> Option<PathBuf> {
    let mut file_buf = vec![0_u16; 4096];
    let max_name = file_buf.len().saturating_sub(1);
    if let Some(name) = spec.default_name {
        for (dst, src) in file_buf
            .iter_mut()
            .take(max_name)
            .zip(name.encode_utf16().filter(|&c| c != 0))
        {
            *dst = src;
        }
    }

    let filter_wide = spec.filter_wide;
    let title_wide = str_to_wide_null(spec.title);
    let clean_ext = spec.def_ext.trim_start_matches('.');
    let def_ext_wide = str_to_wide_null(clean_ext);
    let init_dir_wide = spec
        .initial_dir
        .filter(|p| p.exists())
        .map(|p| os_str_to_wide_null(p.as_os_str()));

    let flags = if spec.save_mode {
        OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY
    } else {
        OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY
    };

    // SAFETY: `GetActiveWindow` takes no pointers and returns the caller thread's active HWND or null.
    let owner = unsafe { GetActiveWindow() };
    let mut ofn = OPENFILENAMEW {
        l_struct_size: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwnd_owner: owner,
        h_instance: std::ptr::null_mut(),
        lpstr_filter: filter_wide.as_ptr(),
        lpstr_custom_filter: std::ptr::null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: file_buf.as_mut_ptr(),
        n_max_file: file_buf.len() as u32,
        lpstr_file_title: std::ptr::null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: init_dir_wide
            .as_ref()
            .map_or(std::ptr::null(), |v| v.as_ptr()),
        lpstr_title: title_wide.as_ptr(),
        flags,
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: def_ext_wide.as_ptr(),
        l_cust_data: 0,
        lpfn_hook: std::ptr::null(),
        lp_template_name: std::ptr::null(),
        pv_reserved: std::ptr::null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    };

    // SAFETY: All pointers stored in `ofn` reference stack/heap buffers owned by this frame
    // (`file_buf`, `filter_wide`, `title_wide`, `def_ext_wide`, `init_dir_wide`) that remain alive
    // and unmoved for the entire duration of the modal `GetSaveFileNameW`/`GetOpenFileNameW` call.
    let ok = unsafe {
        if spec.save_mode {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if ok == 0 {
        return None;
    }

    let end = file_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(file_buf.len());
    let chosen = String::from_utf16_lossy(&file_buf[..end]);
    (!chosen.trim().is_empty()).then(|| PathBuf::from(chosen))
}

/// Opens the native Windows Explorer "Save As" dialog and returns the selected path, if confirmed.
pub fn pick_save_file(
    title: &str,
    filter_label: &str,
    ext: &str,
    default_name: &str,
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    run_file_dialog(FileDialogSpec {
        save_mode: true,
        title,
        filter_wide: build_filter_wide(filter_label, ext),
        def_ext: ext,
        default_name: Some(default_name),
        initial_dir,
    })
}

/// Opens the native Windows Explorer "Open File" dialog and returns the selected path, if confirmed.
pub fn pick_open_file(
    title: &str,
    filter_label: &str,
    ext: &str,
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    pick_open_file_exts(title, filter_label, &[ext], initial_dir)
}

/// Opens the native Windows Explorer "Open File" dialog matching any extension in `exts`.
pub fn pick_open_file_exts(
    title: &str,
    filter_label: &str,
    exts: &[&str],
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    let patterns: Vec<String> = exts
        .iter()
        .map(|ext| format!("*.{}", ext.trim_start_matches('.')))
        .collect();
    let pattern_refs: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let filter_wide = if pattern_refs.is_empty() {
        build_filter_wide(filter_label, "*")
    } else {
        build_multi_filter_wide(filter_label, &pattern_refs)
    };
    let def_ext = exts.first().copied().unwrap_or("");
    run_file_dialog(FileDialogSpec {
        save_mode: false,
        title,
        filter_wide,
        def_ext,
        default_name: None,
        initial_dir,
    })
}

/// Opens the native Windows Explorer dialog filtered for supported raster image formats.
pub fn pick_open_image_file(title: &str, initial_dir: Option<&Path>) -> Option<PathBuf> {
    let filter_wide = build_multi_filter_wide(
        "Image Files",
        &[
            "*.png", "*.jpg", "*.jpeg", "*.jfif", "*.webp", "*.gif", "*.bmp", "*.ico", "*.tif",
            "*.tiff", "*.tga", "*.qoi",
        ],
    );
    run_file_dialog(FileDialogSpec {
        save_mode: false,
        title,
        filter_wide,
        def_ext: "png",
        default_name: None,
        initial_dir,
    })
}

struct ComGuard {
    needs_uninit: bool,
}

impl ComGuard {
    fn init() -> Option<Self> {
        // SAFETY: `CoInitializeEx` initializes COM for the calling thread with no borrowed pointers.
        let hr = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
            )
        };
        if hr == S_OK || hr == S_FALSE {
            Some(Self { needs_uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Some(Self {
                needs_uninit: false,
            })
        } else {
            None
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.needs_uninit {
            // SAFETY: Matches a successful `CoInitializeEx` call on the same thread.
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct ComObject {
    ptr: *mut std::ffi::c_void,
}

impl ComObject {
    /// # Safety
    /// `ptr` must be a non-null, owning COM interface pointer with at least `vtbl_slot` slots.
    unsafe fn vtable_slot(&self, slot: usize) -> usize {
        // SAFETY: Every COM object starts with a pointer to its interface vtable.
        unsafe {
            let vtbl = *(self.ptr as *const *const usize);
            *vtbl.add(slot)
        }
    }
}

impl Drop for ComObject {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `self.ptr` is an owned COM pointer whose vtable begins with `IUnknown` (`Release` at slot 2).
            unsafe {
                let release_addr = self.vtable_slot(VTBL_IUNKNOWN_RELEASE);
                let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
                    std::mem::transmute(release_addr);
                let _ = release(self.ptr);
            }
        }
    }
}

struct CoTaskMemWideGuard(*mut u16);

impl Drop for CoTaskMemWideGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was allocated by COM via `CoTaskMemAlloc` (`IShellItem::GetDisplayName`).
            unsafe {
                CoTaskMemFree(self.0.cast());
            }
        }
    }
}

fn create_file_open_dialog() -> Option<ComObject> {
    let mut raw: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `CLSID_FILE_OPEN_DIALOG` and `IID_I_FILE_DIALOG` are valid GUIDs and `raw` is a valid out-pointer.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_FILE_OPEN_DIALOG,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_I_FILE_DIALOG,
            &mut raw,
        )
    };
    (hr >= 0 && !raw.is_null()).then_some(ComObject { ptr: raw })
}

fn configure_folder_dialog(dialog: &ComObject, title: &str, initial_dir: Option<&Path>) -> bool {
    let mut opts: u32 = 0;
    // SAFETY: `dialog` is an initialized `IFileDialog`; slots 10 (`GetOptions`), 9 (`SetOptions`),
    // and 17 (`SetTitle`) match the `IFileDialog_Vtbl` ABI.
    unsafe {
        let get_options: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32 =
            std::mem::transmute(dialog.vtable_slot(VTBL_IFILE_DIALOG_GET_OPTIONS));
        if get_options(dialog.ptr, &mut opts) < 0 {
            return false;
        }
        let set_options: unsafe extern "system" fn(*mut std::ffi::c_void, u32) -> i32 =
            std::mem::transmute(dialog.vtable_slot(VTBL_IFILE_DIALOG_SET_OPTIONS));
        if set_options(
            dialog.ptr,
            opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR,
        ) < 0
        {
            return false;
        }
        let title_wide = str_to_wide_null(title);
        let set_title: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16) -> i32 =
            std::mem::transmute(dialog.vtable_slot(VTBL_IFILE_DIALOG_SET_TITLE));
        let _ = set_title(dialog.ptr, title_wide.as_ptr());
    }

    if let Some(dir) = initial_dir.filter(|p| p.exists()) {
        let dir_wide = os_str_to_wide_null(dir.as_os_str());
        let mut item_raw: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `dir_wide` is NUL-terminated UTF-16 and `item_raw` receives an owned `IShellItem`.
        let hr = unsafe {
            SHCreateItemFromParsingName(
                dir_wide.as_ptr(),
                std::ptr::null_mut(),
                &IID_I_SHELL_ITEM,
                &mut item_raw,
            )
        };
        if hr >= 0 && !item_raw.is_null() {
            let folder_item = ComObject { ptr: item_raw };
            // SAFETY: `SetFolder` is at slot 12 on `IFileDialog` and borrows `folder_item.ptr` for this call.
            unsafe {
                let set_folder: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                ) -> i32 = std::mem::transmute(dialog.vtable_slot(VTBL_IFILE_DIALOG_SET_FOLDER));
                let _ = set_folder(dialog.ptr, folder_item.ptr);
            }
        }
    }
    true
}

fn read_shell_item_path(item: &ComObject) -> Option<PathBuf> {
    let mut raw_str: *mut u16 = std::ptr::null_mut();
    // SAFETY: `item` is an `IShellItem`; `GetDisplayName` is at slot 5 and writes a `CoTaskMemAlloc` UTF-16 string.
    let hr = unsafe {
        let get_display_name: unsafe extern "system" fn(
            *mut std::ffi::c_void,
            i32,
            *mut *mut u16,
        ) -> i32 = std::mem::transmute(item.vtable_slot(VTBL_ISHELL_ITEM_GET_DISPLAY_NAME));
        get_display_name(item.ptr, SIGDN_FILESYSPATH, &mut raw_str)
    };
    let _free_guard = CoTaskMemWideGuard(raw_str);
    if hr < 0 || raw_str.is_null() {
        return None;
    }

    let mut len = 0usize;
    while len < 32_768 {
        // SAFETY: `raw_str` is a valid NUL-terminated UTF-16 string until the NUL terminator is reached.
        let unit = unsafe { *raw_str.add(len) };
        if unit == 0 {
            break;
        }
        len += 1;
    }
    if len == 0 {
        return None;
    }
    // SAFETY: `len` code units preceding the NUL terminator belong to the buffer returned by `GetDisplayName`.
    let slice = unsafe { std::slice::from_raw_parts(raw_str, len) };
    let chosen = String::from_utf16_lossy(slice);
    (!chosen.trim().is_empty()).then(|| PathBuf::from(chosen))
}

/// Opens the native Windows Explorer folder picker dialog and returns the selected directory, if confirmed.
pub fn pick_folder(title: &str, initial_dir: Option<&Path>) -> Option<PathBuf> {
    let _com = ComGuard::init()?;
    let dialog = create_file_open_dialog()?;
    if !configure_folder_dialog(&dialog, title, initial_dir) {
        return None;
    }
    // SAFETY: `GetActiveWindow` takes no pointers; `IModalWindow::Show` is slot 3 and `GetResult` is slot 20.
    let result_item = unsafe {
        let owner = GetActiveWindow();
        let show: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32 =
            std::mem::transmute(dialog.vtable_slot(VTBL_IMODAL_WINDOW_SHOW));
        if show(dialog.ptr, owner) < 0 {
            return None;
        }
        let mut item_raw: *mut std::ffi::c_void = std::ptr::null_mut();
        let get_result: unsafe extern "system" fn(
            *mut std::ffi::c_void,
            *mut *mut std::ffi::c_void,
        ) -> i32 = std::mem::transmute(dialog.vtable_slot(VTBL_IFILE_DIALOG_GET_RESULT));
        if get_result(dialog.ptr, &mut item_raw) < 0 || item_raw.is_null() {
            return None;
        }
        ComObject { ptr: item_raw }
    };
    read_shell_item_path(&result_item)
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: `ClipboardGuard` is created only after `OpenClipboard` succeeded on this thread.
        unsafe {
            CloseClipboard();
        }
    }
}

fn copy_clipboard_hglobal_bytes(format: u32) -> Option<Vec<u8>> {
    // SAFETY: Caller holds an open clipboard via `ClipboardGuard` on the current thread.
    unsafe {
        if IsClipboardFormatAvailable(format) == 0 {
            return None;
        }
        let handle = GetClipboardData(format);
        if handle.is_null() {
            return None;
        }
        let size = GlobalSize(handle);
        if size == 0 || size > MAX_CLIPBOARD_IMAGE_BYTES {
            return None;
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
        GlobalUnlock(handle);
        Some(bytes)
    }
}

fn read_clipboard_registered_image(name: &str, magic: &[u8]) -> Option<Vec<u8>> {
    let wide = str_to_wide_null(name);
    // SAFETY: `wide` is a NUL-terminated UTF-16 string valid for this call.
    let fmt = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
    if fmt == 0 {
        return None;
    }
    let bytes = copy_clipboard_hglobal_bytes(fmt)?;
    if !bytes.starts_with(magic) {
        return None;
    }
    Some(if magic == b"\x89PNG\r\n\x1a\n" {
        trim_png_trailing_bytes(bytes)
    } else {
        bytes
    })
}

fn read_clipboard_webp_format(name: &str) -> Option<Vec<u8>> {
    let wide = str_to_wide_null(name);
    // SAFETY: `wide` is a NUL-terminated UTF-16 string valid for this call.
    let fmt = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
    if fmt == 0 {
        return None;
    }
    let bytes = copy_clipboard_hglobal_bytes(fmt)?;
    (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP").then_some(bytes)
}

fn read_clipboard_hdrop_images() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: Caller holds an open clipboard via `ClipboardGuard` on the current thread.
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP) == 0 {
            return out;
        }
        let h_drop = GetClipboardData(CF_HDROP);
        if h_drop.is_null() {
            return out;
        }
        let count = DragQueryFileW(h_drop, u32::MAX, std::ptr::null_mut(), 0);
        for index in 0..count.min(64) {
            let needed = DragQueryFileW(h_drop, index, std::ptr::null_mut(), 0);
            if needed == 0 || needed > 32_768 {
                continue;
            }
            let mut buf = vec![0_u16; needed as usize + 1];
            let copied = DragQueryFileW(h_drop, index, buf.as_mut_ptr(), buf.len() as u32);
            if copied == 0 {
                continue;
            }
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..copied as usize]));
            if path.is_file() && is_supported_image_extension(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn open_clipboard() -> Option<ClipboardGuard> {
    // SAFETY: `GetActiveWindow` takes no pointers; `OpenClipboard` accepts an HWND or null.
    let opened = unsafe {
        let hwnd = GetActiveWindow();
        if OpenClipboard(hwnd) != 0 {
            true
        } else {
            OpenClipboard(std::ptr::null_mut()) != 0
        }
    };
    opened.then_some(ClipboardGuard)
}

/// Reads plain UTF-16 text (`CF_UNICODETEXT`) from the Windows clipboard, if present.
pub fn read_clipboard_text() -> Option<String> {
    let _guard = open_clipboard()?;
    let bytes = copy_clipboard_hglobal_bytes(CF_UNICODETEXT)?;
    let (pairs, _) = bytes[..bytes.len().min(MAX_CLIPBOARD_TEXT_BYTES)].as_chunks::<2>();
    let mut units = Vec::with_capacity(pairs.len());
    for &[lo, hi] in pairs {
        let unit = u16::from_le_bytes([lo, hi]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let text = String::from_utf16_lossy(&units)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    (!text.is_empty()).then_some(text)
}

/// Writes UTF-16 text (`CF_UNICODETEXT`) onto the Windows clipboard.
pub fn write_clipboard_text(text: &str) -> bool {
    let Some(_guard) = open_clipboard() else {
        return false;
    };
    let mut wide: Vec<u16> = text
        .encode_utf16()
        .filter(|&c| c != 0)
        .take(MAX_CLIPBOARD_TEXT_BYTES / 2)
        .collect();
    wide.push(0);
    let byte_len = wide.len().saturating_mul(2);
    // SAFETY: Caller holds an open clipboard via `ClipboardGuard`; `GlobalAlloc` allocates
    // `byte_len` movable bytes that are copied while locked and transferred to Windows when
    // `SetClipboardData` succeeds (freed explicitly on failure).
    unsafe {
        if EmptyClipboard() == 0 {
            return false;
        }
        let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if hmem.is_null() {
            return false;
        }
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            GlobalFree(hmem);
            return false;
        }
        let raw_bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
        std::ptr::copy_nonoverlapping(raw_bytes.as_ptr(), ptr.cast::<u8>(), raw_bytes.len());
        GlobalUnlock(hmem);
        if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
            GlobalFree(hmem);
            return false;
        }
    }
    true
}

/// Reads ordered candidate image payloads (`PNG`, `WebP`, `JPEG`, `GIF`, `TIFF`,
/// `CF_DIBV5`, `CF_DIB`, `CF_HDROP`) from the Windows clipboard so callers can fall back
/// cleanly if one provider format fails decoding.
pub fn read_clipboard_image_candidates() -> Vec<crate::ClipboardImage> {
    let Some(_guard) = open_clipboard() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    for (name, magic) in [
        ("PNG", &b"\x89PNG\r\n\x1a\n"[..]),
        ("image/png", &b"\x89PNG\r\n\x1a\n"[..]),
        ("image/jpeg", &[0xFF, 0xD8, 0xFF][..]),
        ("JFIF", &[0xFF, 0xD8, 0xFF][..]),
        ("GIF", b"GIF89a"),
        ("GIF", b"GIF87a"),
        ("image/gif", b"GIF89a"),
        ("image/gif", b"GIF87a"),
        ("image/tiff", b"II*\x00"),
        ("image/tiff", b"MM\x00*"),
    ] {
        if let Some(bytes) = read_clipboard_registered_image(name, magic) {
            out.push(crate::ClipboardImage::EncodedBytes(bytes));
        }
    }
    if let Some(webp) = read_clipboard_webp_format("image/webp") {
        out.push(crate::ClipboardImage::EncodedBytes(webp));
    }
    for format in [CF_DIBV5, CF_DIB] {
        if let Some(dib) = copy_clipboard_hglobal_bytes(format)
            && let Some(bmp) = dib_to_bmp_bytes(&dib)
        {
            out.push(crate::ClipboardImage::EncodedBytes(bmp));
        }
    }
    for path in read_clipboard_hdrop_images() {
        out.push(crate::ClipboardImage::FilePath(path));
    }
    out
}

/// Reads the highest-priority image payload (`PNG`, `CF_DIBV5`, `CF_DIB`, or `CF_HDROP`)
/// from the Windows clipboard.
pub fn read_clipboard_image() -> Option<crate::ClipboardImage> {
    read_clipboard_image_candidates().into_iter().next()
}

const WH_GETMESSAGE: i32 = 3;
const WH_CALLWNDPROC: i32 = 4;

fn to_raw_packet(info: &POINTER_PEN_INFO) -> RawPenPacket {
    RawPenPacket {
        pointer_id: info.pointer_info.pointer_id,
        pointer_flags: info.pointer_info.pointer_flags,
        pen_flags: info.pen_flags,
        pen_mask: info.pen_mask,
        pressure: info.pressure,
        rotation: info.rotation,
        tilt_x: info.tilt_x,
        tilt_y: info.tilt_y,
        pixel_x: info.pointer_info.pt_pixel_location.x,
        pixel_y: info.pointer_info.pt_pixel_location.y,
        dw_time: info.pointer_info.dw_time,
    }
}

fn read_pen_history(pointer_id: u32) -> Vec<RawPenPacket> {
    // SAFETY: `buf` is stack-allocated and zero-initialized; `GetPointerPenInfoHistory` writes at
    // most `MAX_HISTORY` entries into `buf` and returns non-zero on success.
    let history_packets = unsafe {
        let mut buf: [POINTER_PEN_INFO; MAX_HISTORY] = std::mem::zeroed();
        let mut count = MAX_HISTORY as u32;
        if GetPointerPenInfoHistory(pointer_id, &mut count, buf.as_mut_ptr()) != 0 && count > 0 {
            let n = (count as usize).min(MAX_HISTORY);
            // Windows returns history newest-first; reverse to deliver oldest-to-newest.
            Some(buf[..n].iter().rev().map(to_raw_packet).collect::<Vec<_>>())
        } else {
            None
        }
    };
    if let Some(pkts) = history_packets {
        return pkts;
    }

    // SAFETY: `info` is stack-allocated and zero-initialized and outlives the call.
    unsafe {
        let mut info: POINTER_PEN_INFO = std::mem::zeroed();
        if GetPointerPenInfo(pointer_id, &mut info) != 0 {
            vec![to_raw_packet(&info)]
        } else {
            Vec::new()
        }
    }
}

fn clear_active_pen() {
    BARREL_DOWN.store(false, Ordering::Relaxed);
    SECONDARY_BARREL_DOWN.store(false, Ordering::Relaxed);
    PASTE_REQUESTED.store(false, Ordering::Relaxed);
    PEN_ID.store(NO_PEN, Ordering::Relaxed);
    PEN_END.store(0, Ordering::Relaxed);
    HAS_PEN_TIME.store(false, Ordering::Relaxed);
    let mut guard = SAMPLE_STATE.lock().unwrap_or_else(PoisonError::into_inner);
    guard.latest = None;
}

fn is_vk_down(vk: i32) -> bool {
    // SAFETY: `GetKeyState` takes a virtual-key integer and reads thread key state with no pointers.
    unsafe { (GetKeyState(vk) as u16 & 0x8000) != 0 }
}

fn maybe_clear_stale_pen_on_mouse(msg_time_ms: u32) {
    let pen_id = PEN_ID.load(Ordering::Relaxed);
    if pen_id == NO_PEN {
        return;
    }
    // SAFETY: `GetMessageExtraInfo` reads thread-local metadata for the current queued message.
    let extra = unsafe { GetMessageExtraInfo() };
    let is_promoted = is_pointer_promoted_mouse_extra_info(extra);
    if is_promoted {
        return;
    }
    // SAFETY: `info` is stack-allocated and zero-initialized before querying `GetPointerPenInfo`.
    let os_alive = unsafe {
        let mut info: POINTER_PEN_INFO = std::mem::zeroed();
        GetPointerPenInfo(pen_id, &mut info) != 0
    };
    let elapsed = HAS_PEN_TIME
        .load(Ordering::Relaxed)
        .then(|| msg_time_ms.wrapping_sub(LAST_PEN_TIME_MS.load(Ordering::Relaxed)));
    if should_clear_stale_pen_on_mouse(true, is_promoted, os_alive, elapsed) {
        clear_active_pen();
    }
}

fn handle_message(message: u32, w_param: usize, read_history: bool, msg_time_ms: Option<u32>) {
    if !matches!(
        message,
        WM_POINTERDOWN
            | WM_POINTERUPDATE
            | WM_POINTERUP
            | WM_POINTERENTER
            | WM_POINTERLEAVE
            | WM_POINTERCAPTURECHANGED
    ) {
        return;
    }
    let prev = {
        let guard = SAMPLE_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        PenState {
            barrel: BARREL_DOWN.load(Ordering::Relaxed),
            barrel_secondary: SECONDARY_BARREL_DOWN.load(Ordering::Relaxed),
            end: decode_end(PEN_END.load(Ordering::Relaxed)),
            pen_id: Some(PEN_ID.load(Ordering::Relaxed)).filter(|&id| id != NO_PEN),
            contact_end: decode_end(CONTACT_END.load(Ordering::Relaxed)),
            latest: guard.latest,
        }
    };
    let (next, samples) = prev.step(message, w_param, |id| {
        if read_history {
            read_pen_history(id)
        } else {
            Vec::new()
        }
    });
    if !prev.barrel && next.barrel {
        BARREL_PRESS_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if !prev.barrel_secondary && next.barrel_secondary {
        SECONDARY_BARREL_PRESS_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    BARREL_DOWN.store(next.barrel, Ordering::Relaxed);
    SECONDARY_BARREL_DOWN.store(next.barrel_secondary, Ordering::Relaxed);
    PEN_END.store(encode_end(next.end), Ordering::Relaxed);
    PEN_ID.store(next.pen_id.unwrap_or(NO_PEN), Ordering::Relaxed);
    CONTACT_END.store(encode_end(next.contact_end), Ordering::Relaxed);
    if next.pen_id.is_some() {
        if let Some(t) = msg_time_ms
            && matches!(message, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERENTER)
        {
            LAST_PEN_TIME_MS.store(t, Ordering::Relaxed);
            HAS_PEN_TIME.store(true, Ordering::Relaxed);
        }
    } else {
        HAS_PEN_TIME.store(false, Ordering::Relaxed);
    }

    let mut guard = SAMPLE_STATE.lock().unwrap_or_else(PoisonError::into_inner);
    guard.latest = next.latest;
    for s in samples {
        if guard.queue.len() >= MAX_QUEUED_SAMPLES {
            guard.queue.pop_front();
        }
        guard.queue.push_back(s);
    }
}

unsafe extern "system" fn get_msg_proc(code: i32, w_param: usize, l_param: isize) -> isize {
    // Only record queued pointer messages when removed (`PM_REMOVE`); `PM_NOREMOVE` peeks
    // visit the same queued message multiple times and would duplicate history entries.
    if code >= 0 && (w_param & PM_REMOVE) != 0 && l_param != 0 {
        // SAFETY: Win32 guarantees `l_param` points to a valid `MSG` when `code >= 0`.
        let msg = unsafe { &*(l_param as *const MSG) };
        if msg.message == WM_KEYDOWN {
            let initial_press = (msg.l_param & (1 << 30)) == 0;
            let ctrl_v = msg.w_param == VK_V && is_vk_down(VK_CONTROL) && !is_vk_down(VK_MENU);
            let shift_insert = msg.w_param == VK_INSERT && is_vk_down(VK_SHIFT);
            if initial_press && (ctrl_v || shift_insert) {
                PASTE_REQUESTED.store(true, Ordering::Relaxed);
            }
        } else if matches!(
            msg.message,
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL
        ) {
            maybe_clear_stale_pen_on_mouse(msg.time);
        } else {
            handle_message(msg.message, msg.w_param, true, Some(msg.time));
        }
    }
    // SAFETY: Passing through to the next hook in the chain per Win32 docs.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, w_param, l_param) }
}

unsafe extern "system" fn call_wnd_proc(code: i32, w_param: usize, l_param: isize) -> isize {
    if code >= 0 && l_param != 0 {
        // SAFETY: Win32 guarantees `l_param` points to a valid `CWPSTRUCT` when `code >= 0`.
        let msg = unsafe { &*(l_param as *const CWPSTRUCT) };
        match msg.message {
            WM_POINTERENTER => {
                // SAFETY: `GetMessageTime` takes no pointers and returns the Win32 message tick.
                let t = unsafe { GetMessageTime() } as u32;
                handle_message(msg.message, msg.w_param, true, Some(t));
            }
            WM_POINTERLEAVE | WM_POINTERCAPTURECHANGED => {
                handle_message(msg.message, msg.w_param, false, None);
            }
            WM_POINTERDEVICECHANGE | WM_POINTERDEVICEINRANGE => {
                DEVICES_DIRTY.store(true, Ordering::Relaxed);
            }
            WM_KILLFOCUS => clear_active_pen(),
            _ => {}
        }
    }
    // SAFETY: Passing through to the next hook in the chain per Win32 docs.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, w_param, l_param) }
}

/// Thread-local Windows pointer hook handle. Removed cleanly on drop.
pub struct Monitor {
    hooks: [*mut std::ffi::c_void; 2],
}

impl Monitor {
    pub fn install() -> Result<Self, crate::Error> {
        // SAFETY: `GetCurrentThreadId` has no preconditions.
        let tid = unsafe { GetCurrentThreadId() };
        // SAFETY: Installs thread-local `WH_GETMESSAGE` and `WH_CALLWNDPROC` hooks on the current UI thread.
        let (h_get, h_wnd) = unsafe {
            (
                SetWindowsHookExW(WH_GETMESSAGE, get_msg_proc, std::ptr::null_mut(), tid),
                SetWindowsHookExW(WH_CALLWNDPROC, call_wnd_proc, std::ptr::null_mut(), tid),
            )
        };
        if h_get.is_null() {
            if !h_wnd.is_null() {
                // SAFETY: `h_wnd` is non-null from `SetWindowsHookExW`.
                unsafe {
                    UnhookWindowsHookEx(h_wnd);
                }
            }
            return Err(crate::Error::Platform(
                "SetWindowsHookExW(WH_GETMESSAGE) returned null".into(),
            ));
        }
        Ok(Self {
            hooks: [h_get, h_wnd],
        })
    }

    pub fn tracks_leave(&self) -> bool {
        !self.hooks[1].is_null()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        for hook in self.hooks {
            if !hook.is_null() {
                // SAFETY: `hook` is a non-null handle returned by `SetWindowsHookExW` and unhooked once.
                unsafe {
                    UnhookWindowsHookEx(hook);
                }
            }
        }
        clear_active_pen();
        BARREL_PRESS_COUNT.store(0, Ordering::Relaxed);
        SECONDARY_BARREL_PRESS_COUNT.store(0, Ordering::Relaxed);
        CONTACT_END.store(0, Ordering::Relaxed);
        let mut guard = SAMPLE_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        guard.queue.clear();
    }
}

pub fn is_barrel_button_down() -> bool {
    BARREL_DOWN.load(Ordering::Relaxed)
}

pub fn is_secondary_barrel_button_down() -> bool {
    SECONDARY_BARREL_DOWN.load(Ordering::Relaxed)
}

/// Returns and clears the count of primary barrel button presses observed since the last poll.
pub fn take_barrel_press_count() -> u32 {
    BARREL_PRESS_COUNT.swap(0, Ordering::Relaxed)
}

/// Returns and clears the count of secondary barrel button presses observed since the last poll.
pub fn take_secondary_barrel_press_count() -> u32 {
    SECONDARY_BARREL_PRESS_COUNT.swap(0, Ordering::Relaxed)
}

/// Returns and clears whether Windows reported a pointer device plug/unplug change.
pub fn take_pen_devices_dirty() -> bool {
    DEVICES_DIRTY.swap(false, Ordering::Relaxed)
}

/// Queries Windows for connected integrated or external pen digitizers.
pub fn connected_pen_devices() -> Vec<crate::ConnectedPenDevice> {
    let mut out = Vec::new();
    let mut count: u32 = 0;
    // SAFETY: Passing `std::ptr::null_mut()` with a valid `&mut count` queries the device count per Win32 docs.
    let ok = unsafe { GetPointerDevices(&mut count, std::ptr::null_mut()) };
    if ok != 0 && count > 0 {
        let n = (count as usize).min(MAX_POINTER_DEVICES);
        // SAFETY: `POINTER_DEVICE_INFO` is a plain C POD struct where all-zero bits are valid.
        let mut buf: Vec<POINTER_DEVICE_INFO> = vec![unsafe { std::mem::zeroed() }; n];
        let mut read_count = n as u32;
        // SAFETY: `buf` has capacity and length `n >= read_count`, and remains live for this call.
        if unsafe { GetPointerDevices(&mut read_count, buf.as_mut_ptr()) } != 0 {
            let returned = (read_count as usize).min(n);
            for dev in &buf[..returned] {
                let kind = match dev.pointer_device_type {
                    POINTER_DEVICE_TYPE_INTEGRATED_PEN => Some(crate::PenDeviceKind::Integrated),
                    POINTER_DEVICE_TYPE_EXTERNAL_PEN => Some(crate::PenDeviceKind::External),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let name = crate::win_ink::sanitize_pen_device_name(
                        &dev.product_string,
                        kind == crate::PenDeviceKind::External,
                    );
                    out.push(crate::ConnectedPenDevice { name, kind });
                }
            }
        }
    }

    if out.is_empty() {
        // SAFETY: `GetSystemMetrics` takes an integer metric index and uses no pointers.
        let metrics = unsafe { GetSystemMetrics(SM_DIGITIZER) };
        if (metrics & NID_READY) != 0 {
            if (metrics & NID_INTEGRATED_PEN) != 0 {
                out.push(crate::ConnectedPenDevice {
                    name: "Integrated Stylus".to_string(),
                    kind: crate::PenDeviceKind::Integrated,
                });
            }
            if (metrics & NID_EXTERNAL_PEN) != 0 {
                out.push(crate::ConnectedPenDevice {
                    name: "External Pen Tablet".to_string(),
                    kind: crate::PenDeviceKind::External,
                });
            }
        }
    }

    out
}

/// Returns and clears the queued Windows paste shortcut signal (`Ctrl+V` or `Shift+Insert`).
pub fn take_paste_requested() -> bool {
    PASTE_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Active Win32 pointer ID while a stylus is in digitizer proximity (`None` after `WM_POINTERLEAVE`).
pub fn pen_pointer_id() -> Option<u64> {
    let id = PEN_ID.load(Ordering::Relaxed);
    (id != NO_PEN).then_some(u64::from(id))
}

pub fn pen_end() -> Option<bool> {
    decode_end(PEN_END.load(Ordering::Relaxed))
}

pub fn take_pen_end() -> Option<bool> {
    decode_end(CONTACT_END.swap(0, Ordering::Relaxed)).or_else(pen_end)
}

pub fn latest_sample() -> Option<Sample> {
    SAMPLE_STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .latest
}

pub fn drain_samples() -> Vec<Sample> {
    SAMPLE_STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .queue
        .drain(..)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_device_info_layout_matches_winuser_abi() {
        let expected = if cfg!(target_pointer_width = "64") {
            1080
        } else {
            1064
        };
        assert_eq!(std::mem::size_of::<POINTER_DEVICE_INFO>(), expected);
    }

    #[test]
    fn connected_pen_devices_queries_cleanly_without_panicking() {
        let devices = connected_pen_devices();
        for dev in devices {
            assert!(!dev.name.is_empty());
        }
    }

    #[test]
    fn folder_dialog_com_guid_and_vtable_constants_match_abi() {
        assert_eq!(std::mem::size_of::<ComGuid>(), 16);
        assert_eq!(CLSID_FILE_OPEN_DIALOG.data1, 0xdc1c_5a9c);
        assert_eq!(IID_I_FILE_DIALOG.data1, 0x42f8_5136);
        assert_eq!(IID_I_SHELL_ITEM.data1, 0x4382_6d1e);
        assert_eq!(IUNKNOWN_VTBL_LEN, 3);
        assert_eq!(IFILE_DIALOG_VTBL_LEN, 27);
        assert_eq!(ISHELL_ITEM_VTBL_LEN, 8);
    }
}
