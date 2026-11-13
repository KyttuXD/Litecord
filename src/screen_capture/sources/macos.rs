//! macOS screen capture through CoreGraphics.
//!
//! `CGDisplayCreateImage` exists on every macOS this build runs on (10.12 and later) and
//! needs no GPU feature, so it also works on old Intel machines. Each call returns the
//! whole display as BGRA, which is scaled into the stream canvas. The mouse pointer is
//! not part of the picture. From macOS 10.15 the system asks the user for the Screen
//! Recording permission; until it is granted the picture shows only the desktop.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::super::types::{CapturableWindowItem, MonitorItemInfo};
use super::super::video_pipeline::fit_bgra_to_canvas;

type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGGetActiveDisplayList(max_displays: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
    fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
    fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
    fn CGDisplayCreateImage(display: CGDirectDisplayID) -> *mut c_void;
    fn CGImageGetWidth(image: *mut c_void) -> usize;
    fn CGImageGetHeight(image: *mut c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *mut c_void) -> usize;
    fn CGImageGetBitsPerPixel(image: *mut c_void) -> usize;
    fn CGImageGetDataProvider(image: *mut c_void) -> *mut c_void;
    fn CGDataProviderCopyData(provider: *mut c_void) -> *const c_void;
    fn CGImageRelease(image: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFRelease(cf: *const c_void);
}

/// Which display the running stream captures (index in `list_screens`).
static SCREEN_INDEX: AtomicUsize = AtomicUsize::new(0);
static ACCESS_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn set_screen_index(index: usize) {
    SCREEN_INDEX.store(index, Ordering::Relaxed);
}

fn active_displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; 16];
    let mut count: u32 = 0;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err != 0 || count == 0 {
        return vec![unsafe { CGMainDisplayID() }];
    }
    ids[..(count as usize).min(ids.len())].to_vec()
}

/// Asks for the Screen Recording permission once (macOS 10.15 and later). The two
/// functions do not exist on older systems, so they are looked up at run time.
fn request_screen_access_once() {
    if ACCESS_REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let preflight = libc::dlsym(libc::RTLD_DEFAULT, b"CGPreflightScreenCaptureAccess\0".as_ptr() as *const libc::c_char);
        let request = libc::dlsym(libc::RTLD_DEFAULT, b"CGRequestScreenCaptureAccess\0".as_ptr() as *const libc::c_char);
        if preflight.is_null() || request.is_null() {
            return;
        }
        let preflight: unsafe extern "C" fn() -> bool = std::mem::transmute(preflight);
        let request: unsafe extern "C" fn() -> bool = std::mem::transmute(request);
        if !preflight() {
            log::warn!("🖥️ [MACOS CAPTURE] No Screen Recording permission: asking the system (System Preferences > Security & Privacy).");
            let _ = request();
        }
    }
}

pub fn list_screens() -> Vec<MonitorItemInfo> {
    let main = unsafe { CGMainDisplayID() };
    active_displays()
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            let (w, h) = unsafe { (CGDisplayPixelsWide(id), CGDisplayPixelsHigh(id)) };
            let is_primary = id == main;
            MonitorItemInfo {
                id: i as i32,
                name: if is_primary { format!("Screen {} (Primary)", i + 1) } else { format!("Screen {}", i + 1) },
                resolution: format!("{} × {}", w, h),
                is_primary,
                hwnd: id as isize,
            }
        })
        .collect()
}

/// Single windows are not captured on macOS yet: only whole displays.
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    Vec::new()
}

pub fn capture_screen_rgb(_target_hwnd: isize, target_w: u32, target_h: u32, _target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    request_screen_access_once();

    let displays = active_displays();
    let display = displays
        .get(SCREEN_INDEX.load(Ordering::Relaxed))
        .copied()
        .unwrap_or_else(|| unsafe { CGMainDisplayID() });

    let grab_start = std::time::Instant::now();
    unsafe {
        let image = CGDisplayCreateImage(display);
        if image.is_null() {
            return None;
        }
        let w = CGImageGetWidth(image);
        let h = CGImageGetHeight(image);
        let stride = CGImageGetBytesPerRow(image);
        let bpp = CGImageGetBitsPerPixel(image);
        let provider = CGImageGetDataProvider(image);
        let data = if provider.is_null() { std::ptr::null() } else { CGDataProviderCopyData(provider) };
        if data.is_null() {
            CGImageRelease(image);
            return None;
        }
        let grab_us = grab_start.elapsed().as_micros();

        let ptr = CFDataGetBytePtr(data);
        let len = CFDataGetLength(data).max(0) as usize;
        let needed = stride.saturating_mul(h);
        let mut result = None;
        if bpp == 32 && w > 0 && h > 0 && stride >= w * 4 && !ptr.is_null() && len >= needed {
            let pix_start = std::time::Instant::now();
            // Exactly `stride * h` bytes: the scaler derives the row pitch from the length.
            let src = std::slice::from_raw_parts(ptr, needed);
            let total = (target_w as usize) * (target_h as usize) * 4;
            if out_bgra.len() != total {
                out_bgra.resize(total, 0);
            }
            fit_bgra_to_canvas(src, w as u32, h as u32, target_w, target_h, out_bgra);
            result = Some((grab_us, pix_start.elapsed().as_micros()));
        }
        CFRelease(data);
        CGImageRelease(image);
        result
    }
}
