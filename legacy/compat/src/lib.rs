//! See Cargo.toml. Call [`init`] first thing in `main`, which also makes sure this crate is
//! linked into the program.
//!
//! How it works: the legacy programs are linked with a flat namespace, so a call to `fcntl`
//! from any Rust code in the program is looked up in the program itself before libSystem.
//! The `fcntl` below takes those calls and passes them on to the real one.

/// `fcntl(fd, F_DUPFD_CLOEXEC)` arrived in OS X 10.7; 10.6 answers EBADF (seen on a Mac Pro
/// 3,1). The Rust standard library uses it for every `try_clone` of a file, socket or
/// kqueue, and tokio cannot start without that. Here it becomes F_DUPFD followed by
/// F_SETFD(FD_CLOEXEC): the same result, without being atomic.
#[cfg(target_os = "macos")]
mod fcntl_shim {
    use libc::{c_char, c_int, c_void};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const F_DUPFD_CLOEXEC: c_int = 67;
    type RealFcntl = unsafe extern "C" fn(c_int, c_int, usize) -> c_int;
    static REAL: AtomicUsize = AtomicUsize::new(0);

    unsafe fn real() -> RealFcntl {
        let mut ptr = REAL.load(Ordering::Relaxed);
        if ptr == 0 {
            // The next `fcntl` after this program's own: the one in libSystem.
            ptr = libc::dlsym(libc::RTLD_NEXT, b"fcntl\0".as_ptr() as *const c_char) as usize;
            if ptr == 0 {
                // Never expected; without the real function nothing can be passed on.
                libc::abort();
            }
            REAL.store(ptr, Ordering::Relaxed);
        }
        std::mem::transmute::<*const c_void, RealFcntl>(ptr as *const c_void)
    }

    /// The third argument of `fcntl` is an integer or a pointer; on x86_64 both travel in
    /// the same register, so one pointer-sized parameter stands for either.
    #[no_mangle]
    pub unsafe extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: usize) -> c_int {
        let real = real();
        if cmd != F_DUPFD_CLOEXEC {
            return real(fd, cmd, arg);
        }
        let first = real(fd, cmd, arg);
        if first >= 0 {
            return first; // OS X 10.7 and later.
        }
        let new_fd = real(fd, libc::F_DUPFD, arg);
        if new_fd >= 0 {
            real(new_fd, libc::F_SETFD, libc::FD_CLOEXEC as usize);
        }
        new_fd
    }
}

/// Keeps this crate (and its `fcntl`) in the program.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        let _ = fcntl_shim::fcntl as unsafe extern "C" fn(libc::c_int, libc::c_int, usize) -> libc::c_int;
    }
}
