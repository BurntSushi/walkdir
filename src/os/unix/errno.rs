// This was mostly lifted from the standard library's sys module. The main
// difference is that we need to use a C shim to get access to errno on
// DragonflyBSD, since the #[thread_local] attribute isn't stable (as of Rust
// 1.34).

use libc::c_int;

extern "C" {
    #[cfg_attr(
        any(
            target_os = "linux",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "l4re",
        ),
        link_name = "__errno_location"
    )]
    #[cfg_attr(
        any(
            target_os = "bitrig",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "android",
            target_os = "hermit",
        ),
        link_name = "__errno"
    )]
    #[cfg_attr(target_os = "dragonfly", link_name = "errno_location")]
    #[cfg_attr(target_os = "solaris", link_name = "___errno")]
    #[cfg_attr(
        any(target_os = "macos", target_os = "ios", target_os = "freebsd",),
        link_name = "__error"
    )]
    #[cfg_attr(target_os = "haiku", link_name = "_errnop")]
    fn errno_location() -> *mut c_int;
}

/// Returns the platform-specific value of errno.
pub fn errno() -> i32 {
    unsafe { (*errno_location()) as i32 }
}

/// Clears the platform-specific value of errno to 0.
pub fn clear() {
    unsafe {
        *errno_location() = 0;
    }
}
