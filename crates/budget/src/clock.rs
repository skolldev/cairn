//! One clock for every process on the machine, and for the OS's frame times, so that a time
//! stamped in the harness compares with one stamped in Cairn or by the OS.
//!
//! macOS: `CLOCK_UPTIME_RAW`, the `mach_absolute_time` clock that `CACurrentMediaTime` and Metal's
//! presented times read. Windows: `QueryPerformanceCounter`, which PresentMon's `--qpc_time`
//! reports. Elsewhere: `CLOCK_MONOTONIC`, because the harness's numbers don't count there.

/// Nanoseconds on the machine-wide clock.
#[cfg(unix)]
pub fn now_ns() -> u64 {
    #[cfg(target_os = "macos")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_os = "macos"))]
    let clock = libc::CLOCK_MONOTONIC;
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid timespec for the call to write.
    unsafe { libc::clock_gettime(clock, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

#[cfg(windows)]
pub fn now_ns() -> u64 {
    let mut ticks = 0;
    // SAFETY: `ticks` is a valid i64 for the call to write, and this can't fail since Windows XP.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut ticks) };
    ticks_to_ns(ticks as u64, ticks_per_second())
}

/// How fast PresentMon's QPC values tick on this machine. 1 where there is no such counter.
#[cfg(windows)]
pub fn ticks_per_second() -> u64 {
    let mut frequency = 0;
    // SAFETY: `frequency` is a valid i64 for the call to write.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency) };
    frequency as u64
}

#[cfg(not(windows))]
pub fn ticks_per_second() -> u64 {
    1
}

/// Converts counter ticks to nanoseconds without overflowing on a long uptime.
pub fn ticks_to_ns(ticks: u64, ticks_per_second: u64) -> u64 {
    (ticks as u128 * 1_000_000_000 / ticks_per_second as u128) as u64
}
