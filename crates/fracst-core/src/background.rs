//! Running long jobs (video keyframes, say) in the background, so the
//! computer stays responsive while they use the CPU: their threads run at a
//! low priority and leave a core free.

/// Lower the calling thread's priority. Does nothing on platforms other than
/// Windows and Linux.
pub fn lower_thread_priority() {
    #[cfg(windows)]
    // SAFETY: changes only the priority of the calling thread.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
    }
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux the nice value is per thread; this changes the calling thread's.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

/// Threads for a background job: all the logical CPUs but one core's two.
pub fn threads() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    cpus.saturating_sub(2).max(1)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn lowers_the_priority_of_the_calling_thread_only() {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_LOWEST};
        let priority = || unsafe { GetThreadPriority(GetCurrentThread()) };
        let before = priority();
        let lowered = std::thread::spawn(move || {
            super::lower_thread_priority();
            priority()
        });
        assert_eq!(lowered.join().unwrap(), THREAD_PRIORITY_LOWEST);
        assert_eq!(priority(), before);
    }

    #[test]
    fn leaves_a_core_free() {
        let cpus = std::thread::available_parallelism().unwrap().get();
        assert_eq!(super::threads(), cpus.saturating_sub(2).max(1));
    }
}
