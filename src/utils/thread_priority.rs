//! Scheduling priority for the threads a screen share depends on.
//!
//! With every core busy (a CPU-heavy game, a compile), threads at normal priority wait
//! their turn like any other and the stream drops frames: in a test with the CPU at
//! 100% the viewer fell to ~46 FPS with freezes. One step above normal lets the
//! capture, the encoder and the senders run first; they use little CPU, so the game
//! loses almost nothing.

/// Raises the calling thread one step above normal.
pub fn raise_current_thread() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) == 0 {
            log::debug!("SetThreadPriority failed for {:?}", std::thread::current().name());
        }
    }

    // macOS: ask the scheduler for the "user initiated" QoS class (0x19), which sits above
    // the default class. Best effort; the return value is an errno-style code.
    #[cfg(target_os = "macos")]
    unsafe {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_USER_INITIATED: u32 = 0x19;
        if pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0) != 0 {
            log::debug!("pthread_set_qos_class_self_np failed for {:?}", std::thread::current().name());
        }
    }

    // Linux: nice -1 for this thread only (on Linux setpriority(PRIO_PROCESS, tid) is
    // per-thread). Raising priority needs RLIMIT_NICE or CAP_SYS_NICE, so EPERM/EACCES is
    // expected for normal users and ignored.
    #[cfg(target_os = "linux")]
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid);
        if tid > 0 && libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, -1) != 0 {
            let err = std::io::Error::last_os_error().raw_os_error();
            if err != Some(libc::EPERM) && err != Some(libc::EACCES) {
                log::debug!("setpriority failed for {:?}: {:?}", std::thread::current().name(), err);
            }
        }
    }
}
