use std::io;

/// Linux scheduler policy for a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    /// Normal time-sharing scheduler (`SCHED_OTHER`). Priority is always 0;
    /// "niceness" controls relative weight within the CFS.
    Normal,
    /// First-in, first-out real-time scheduler (`SCHED_FIFO`).
    /// Priority 1–99 (higher = more urgent). Never preempted by SCHED_OTHER.
    /// Requires `CAP_SYS_NICE` or `RLIMIT_RTPRIO > 0`.
    Fifo,
    /// Round-robin real-time scheduler (`SCHED_RR`).
    /// Like `Fifo` but threads at the same priority share the CPU in slices.
    RoundRobin,
}

// ── Linux ──────────────────────────────────────────────────────────────────

/// Set the current thread to `SCHED_FIFO` with the given priority (1–99).
///
/// A FIFO thread runs until it blocks or is preempted by a higher-priority
/// real-time thread. This eliminates involuntary context-switch latency from
/// normal threads.
///
/// # Errors
/// Returns `EPERM` if the process lacks `CAP_SYS_NICE`. On a development
/// machine, grant the binary `CAP_SYS_NICE` or raise `ulimit -r`.
///
/// # Panics
/// Panics if `priority` is outside `1..=99`.
#[cfg(target_os = "linux")]
pub fn set_realtime(priority: i32) -> io::Result<()> {
    assert!((1..=99).contains(&priority), "FIFO priority must be 1–99, got {priority}");
    let param = libc::sched_param { sched_priority: priority };
    let ret = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Restore the current thread to `SCHED_OTHER` (priority 0).
#[cfg(target_os = "linux")]
pub fn set_normal() -> io::Result<()> {
    let param = libc::sched_param { sched_priority: 0 };
    let ret = unsafe { libc::sched_setscheduler(0, libc::SCHED_OTHER, &param) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Return the scheduler policy of the current thread.
#[cfg(target_os = "linux")]
pub fn get_policy() -> io::Result<Policy> {
    let ret = unsafe { libc::sched_getscheduler(0) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    match ret {
        libc::SCHED_FIFO => Ok(Policy::Fifo),
        libc::SCHED_RR => Ok(Policy::RoundRobin),
        _ => Ok(Policy::Normal),
    }
}

// ── Non-Linux stubs ────────────────────────────────────────────────────────

/// Set the current thread to `SCHED_FIFO`. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn set_realtime(priority: i32) -> io::Result<()> {
    assert!((1..=99).contains(&priority), "FIFO priority must be 1–99, got {priority}");
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "RT scheduling requires Linux",
    ))
}

/// Restore the current thread to `SCHED_OTHER`. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn set_normal() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Scheduler control requires Linux",
    ))
}

/// Return the scheduler policy of the current thread. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn get_policy() -> io::Result<Policy> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Scheduler query requires Linux",
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn set_realtime_priority_zero_panics() {
        let _ = set_realtime(0);
    }

    #[test]
    #[should_panic]
    fn set_realtime_priority_100_panics() {
        let _ = set_realtime(100);
    }

    /// On Linux: the policy query must succeed. Default for user threads is Normal.
    #[cfg(target_os = "linux")]
    #[test]
    fn get_policy_returns_valid() {
        let policy = get_policy().expect("sched_getscheduler should succeed");
        assert!(
            policy == Policy::Normal || policy == Policy::Fifo || policy == Policy::RoundRobin
        );
    }

    /// On non-Linux: all three functions return Unsupported.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn stubs_return_unsupported() {
        assert_eq!(set_realtime(1).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(set_normal().unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(get_policy().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
