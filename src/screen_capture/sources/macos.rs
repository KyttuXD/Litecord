//! macOS screen capture through CoreGraphics.
//!
//! `CGDisplayCreateImage` exists on every macOS this build runs on (10.12 and later) and
//! needs no GPU feature, so it also works on old Intel machines. Each call returns the
//! whole display as BGRA, which is scaled into the stream canvas. The mouse pointer is
//! not part of the picture. From macOS 10.15 the system asks the user for the Screen
//! Recording permission; until it is granted the picture shows only the desktop.
//! Single windows are listed with `CGWindowListCopyWindowInfo` and captured with
//! `CGWindowListCreateImage`.

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

pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    request_screen_access_once();
    let mut records: Vec<MacWindowRecord> = Vec::new();
    unsafe {
        let array = ffi::CGWindowListCopyWindowInfo(
            ffi::K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | ffi::K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            ffi::K_CG_NULL_WINDOW_ID,
        );
        if array.is_null() {
            return Vec::new();
        }
        let count = ffi::CFArrayGetCount(array);
        for i in 0..count {
            // Borrowed from the array (Get rule): must not be released.
            let dict = ffi::CFArrayGetValueAtIndex(array, i);
            if dict.is_null() {
                continue;
            }
            let id = match dict_i32(dict, ffi::kCGWindowNumber) {
                Some(v) if v > 0 => v as u32,
                _ => continue,
            };
            let size = dict_size(dict);
            records.push(MacWindowRecord {
                id,
                layer: dict_i32(dict, ffi::kCGWindowLayer).unwrap_or(-1),
                pid: dict_i32(dict, ffi::kCGWindowOwnerPID).unwrap_or(-1),
                name: dict_string(dict, ffi::kCGWindowName),
                owner_name: dict_string(dict, ffi::kCGWindowOwnerName),
                width: size.0,
                height: size.1,
                alpha: dict_f64(dict, ffi::kCGWindowAlpha).unwrap_or(1.0),
            });
        }
        // The array came from a Copy function: we own it.
        ffi::CFRelease(array);
    }
    filter_window_records(&records, std::process::id() as i32)
}


pub fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, _target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    request_screen_access_once();
    if target_hwnd != 0 {
        return capture_window_rgb(target_hwnd, target_w, target_h, out_bgra);
    }

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

/// Plain description of one entry returned by `CGWindowListCopyWindowInfo`.
/// Kept free of any platform types so the filtering rules can be unit tested anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct MacWindowRecord {
    pub id: u32,
    pub layer: i32,
    pub pid: i32,
    pub name: String,
    pub owner_name: String,
    pub width: f64,
    pub height: f64,
    pub alpha: f64,
}

/// Smallest window side (in points) offered for sharing.
const MIN_WINDOW_SIDE: f64 = 50.0;

/// Keeps the windows the user can reasonably pick to share: normal layer (0) windows that
/// have a title or an owner name, excluding the app itself. The input order (front to back,
/// as reported by the window server) is preserved and duplicate ids are dropped.
pub fn filter_window_records(records: &[MacWindowRecord], own_pid: i32) -> Vec<CapturableWindowItem> {
    let mut seen: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    for rec in records {
        if rec.layer != 0 || rec.pid == own_pid || rec.id == 0 {
            continue;
        }
        // Skip helper windows nobody can see: tiny or fully transparent.
        if rec.width < MIN_WINDOW_SIDE || rec.height < MIN_WINDOW_SIDE || rec.alpha <= 0.0 {
            continue;
        }
        let name = rec.name.trim();
        let owner = rec.owner_name.trim();
        if name.is_empty() && owner.is_empty() {
            continue;
        }
        if seen.contains(&rec.id) {
            continue;
        }
        seen.push(rec.id);
        let title = if name.is_empty() { owner } else { name };
        let app_name = if owner.is_empty() { "Window" } else { owner };
        out.push(CapturableWindowItem {
            id: rec.id.to_string(),
            title: title.to_string(),
            app_name: app_name.to_string(),
            icon_rgba: None,
        });
    }
    out
}

/// Converts the opaque target id used by the stream pipeline into a CGWindowID.
pub fn window_id_from_target(target: isize) -> Option<u32> {
    u32::try_from(target).ok().filter(|v| *v != 0)
}

mod ffi {
    use std::os::raw::{c_char, c_void};

    pub type CFTypeRef = *const c_void;
    pub type CFIndex = isize;

    pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    pub const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
    pub const K_CF_NUMBER_FLOAT64_TYPE: i32 = 6;

    pub const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1;
    pub const K_CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW: u32 = 1 << 3;
    pub const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
    pub const K_CG_NULL_WINDOW_ID: u32 = 0;
    pub const K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING: u32 = 1;

    // kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little => BGRA bytes in memory.
    pub const BGRA_BITMAP_INFO: u32 = 2 | (2 << 12);

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub fn CFNumberGetTypeID() -> usize;
        pub fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
        pub fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: CFIndex) -> CFTypeRef;
        pub fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        pub fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
        pub fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
        pub fn CFStringGetCString(s: CFTypeRef, buffer: *mut c_char, buffer_size: CFIndex, encoding: u32) -> u8;
        pub fn CFNumberGetValue(number: CFTypeRef, the_type: i32, value_ptr: *mut c_void) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub static kCGWindowNumber: CFTypeRef;
        pub static kCGWindowName: CFTypeRef;
        pub static kCGWindowOwnerName: CFTypeRef;
        pub static kCGWindowOwnerPID: CFTypeRef;
        pub static kCGWindowLayer: CFTypeRef;
        pub static kCGWindowBounds: CFTypeRef;
        pub static kCGWindowAlpha: CFTypeRef;

        pub fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;

        pub fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFTypeRef;
        pub fn CGWindowListCreateImage(screen_bounds: CGRect, list_option: u32, window_id: u32, image_option: u32) -> CFTypeRef;
        pub fn CGColorSpaceCreateDeviceRGB() -> CFTypeRef;
        pub fn CGColorSpaceRelease(space: CFTypeRef);
        pub fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
        ) -> CFTypeRef;
        pub fn CGContextDrawImage(ctx: CFTypeRef, rect: CGRect, image: CFTypeRef);
        pub fn CGContextRelease(ctx: CFTypeRef);
    }
}

/// Reads a CFString dictionary value as a Rust String (empty when missing or not a string).
unsafe fn dict_string(dict: ffi::CFTypeRef, key: ffi::CFTypeRef) -> String {
    let value = ffi::CFDictionaryGetValue(dict, key);
    if value.is_null() || ffi::CFGetTypeID(value) != ffi::CFStringGetTypeID() {
        return String::new();
    }
    let len = ffi::CFStringGetLength(value);
    let max = ffi::CFStringGetMaximumSizeForEncoding(len, ffi::K_CF_STRING_ENCODING_UTF8) + 1;
    if max <= 1 {
        return String::new();
    }
    let mut buf: Vec<u8> = vec![0u8; max as usize];
    let ok = ffi::CFStringGetCString(
        value,
        buf.as_mut_ptr() as *mut std::os::raw::c_char,
        max,
        ffi::K_CF_STRING_ENCODING_UTF8,
    );
    if ok == 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Reads a CFNumber dictionary value as i32 (None when missing or not a number).
unsafe fn dict_i32(dict: ffi::CFTypeRef, key: ffi::CFTypeRef) -> Option<i32> {
    let value = ffi::CFDictionaryGetValue(dict, key);
    if value.is_null() || ffi::CFGetTypeID(value) != ffi::CFNumberGetTypeID() {
        return None;
    }
    let mut out: i32 = 0;
    let ok = ffi::CFNumberGetValue(
        value,
        ffi::K_CF_NUMBER_SINT32_TYPE,
        &mut out as *mut i32 as *mut std::os::raw::c_void,
    );
    if ok == 0 {
        None
    } else {
        Some(out)
    }
}

/// Reads a CFNumber dictionary value as f64 (None when missing or not a number).
unsafe fn dict_f64(dict: ffi::CFTypeRef, key: ffi::CFTypeRef) -> Option<f64> {
    let value = ffi::CFDictionaryGetValue(dict, key);
    if value.is_null() || ffi::CFGetTypeID(value) != ffi::CFNumberGetTypeID() {
        return None;
    }
    let mut out: f64 = 0.0;
    let ok = ffi::CFNumberGetValue(
        value,
        ffi::K_CF_NUMBER_FLOAT64_TYPE,
        &mut out as *mut f64 as *mut std::os::raw::c_void,
    );
    if ok == 0 {
        None
    } else {
        Some(out)
    }
}

/// Reads the window's width and height from its kCGWindowBounds dictionary.
unsafe fn dict_size(dict: ffi::CFTypeRef) -> (f64, f64) {
    let bounds = ffi::CFDictionaryGetValue(dict, ffi::kCGWindowBounds);
    if bounds.is_null() {
        return (0.0, 0.0);
    }
    let mut rect = ffi::CGRect {
        origin: ffi::CGPoint { x: 0.0, y: 0.0 },
        size: ffi::CGSize { width: 0.0, height: 0.0 },
    };
    if ffi::CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) {
        (rect.size.width, rect.size.height)
    } else {
        (0.0, 0.0)
    }
}

fn capture_window_rgb(target_hwnd: isize, target_w: u32, target_h: u32, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    use std::cell::RefCell;

    thread_local! {
        static WINDOW_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::new());
    }

    let window_id = window_id_from_target(target_hwnd)?;
    if target_w == 0 || target_h == 0 {
        return None;
    }

    let t_blt_start = std::time::Instant::now();
    let (src_w, src_h) = unsafe {
        // CGRectNull: let the window server use the window's own bounds.
        let rect_null = ffi::CGRect {
            origin: ffi::CGPoint { x: f64::INFINITY, y: f64::INFINITY },
            size: ffi::CGSize { width: 0.0, height: 0.0 },
        };
        let image = ffi::CGWindowListCreateImage(
            rect_null,
            ffi::K_CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW,
            window_id,
            ffi::K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING,
        );
        if image.is_null() {
            return None;
        }
        let w = CGImageGetWidth(image as *mut c_void);
        let h = CGImageGetHeight(image as *mut c_void);
        if w == 0 || h == 0 || w > 8192 || h > 8192 || w * h > 40_000_000 {
            CGImageRelease(image as *mut c_void);
            return None;
        }

        let drawn = WINDOW_SCRATCH.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let needed = w * h * 4;
            if scratch.len() != needed {
                scratch.resize(needed, 0);
            }
            scratch.iter_mut().for_each(|b| *b = 0);

            let space = ffi::CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                return false;
            }
            let ctx = ffi::CGBitmapContextCreate(
                scratch.as_mut_ptr() as *mut std::os::raw::c_void,
                w,
                h,
                8,
                w * 4,
                space,
                ffi::BGRA_BITMAP_INFO,
            );
            ffi::CGColorSpaceRelease(space);
            if ctx.is_null() {
                return false;
            }
            let dest = ffi::CGRect {
                origin: ffi::CGPoint { x: 0.0, y: 0.0 },
                size: ffi::CGSize { width: w as f64, height: h as f64 },
            };
            ffi::CGContextDrawImage(ctx, dest, image);
            ffi::CGContextRelease(ctx);
            for px in scratch.chunks_exact_mut(4) {
                px[3] = 0xFF;
            }
            true
        });
        CGImageRelease(image as *mut c_void);
        if !drawn {
            return None;
        }
        (w as u32, h as u32)
    };
    let blt_dur_us = t_blt_start.elapsed().as_micros();

    let t_pix_start = std::time::Instant::now();
    let total_bytes = (target_w as usize) * (target_h as usize) * 4;
    if out_bgra.len() != total_bytes {
        out_bgra.resize(total_bytes, 0);
    }
    WINDOW_SCRATCH.with(|cell| {
        let scratch = cell.borrow();
        fit_bgra_to_canvas(&scratch, src_w, src_h, target_w, target_h, out_bgra);
    });
    let pix_dur_us = t_pix_start.elapsed().as_micros();

    Some((blt_dur_us, pix_dur_us))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32, layer: i32, pid: i32, name: &str, owner: &str) -> MacWindowRecord {
        MacWindowRecord { id, layer, pid, name: name.to_string(), owner_name: owner.to_string(), width: 800.0, height: 600.0, alpha: 1.0 }
    }

    #[test]
    fn keeps_only_layer_zero_and_skips_own_app() {
        let recs = vec![
            rec(1, 0, 100, "Docs", "Pages"),
            rec(2, 25, 101, "Menu", "SystemUIServer"),
            rec(3, 0, 42, "Litecord", "litecord"),
            rec(4, 0, 102, "", "Finder"),
            rec(5, 0, 103, "  ", " "),
        ];
        let out = filter_window_records(&recs, 42);
        let ids: Vec<&str> = out.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "4"]);
        assert_eq!(out[0].title, "Docs");
        assert_eq!(out[0].app_name, "Pages");
        assert_eq!(out[1].title, "Finder");
    }

    #[test]
    fn drops_tiny_and_transparent_windows() {
        let mut tiny = rec(1, 0, 1, "A", "X");
        tiny.width = 1.0;
        let mut clear = rec(2, 0, 1, "B", "X");
        clear.alpha = 0.0;
        let ok = rec(3, 0, 1, "C", "X");
        let out = filter_window_records(&[tiny, clear, ok], 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "3");
    }

    #[test]
    fn drops_duplicate_ids_and_zero_ids() {
        let recs = vec![rec(7, 0, 1, "A", "X"), rec(7, 0, 1, "A", "X"), rec(0, 0, 1, "B", "X")];
        assert_eq!(filter_window_records(&recs, 0).len(), 1);
    }

    #[test]
    fn target_id_conversion() {
        assert_eq!(window_id_from_target(0), None);
        assert_eq!(window_id_from_target(-5), None);
        assert_eq!(window_id_from_target(1234), Some(1234));
        #[cfg(target_pointer_width = "64")]
        assert_eq!(window_id_from_target(u32::MAX as isize + 1), None);
    }
}
