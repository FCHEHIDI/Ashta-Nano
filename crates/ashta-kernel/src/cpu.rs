use std::io;

/// A set of CPU cores for thread affinity binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuSet {
    cpus: Vec<usize>,
}

impl CpuSet {
    /// Create a set containing a single CPU core.
    pub fn single(cpu: usize) -> Self {
        Self { cpus: vec![cpu] }
    }

    /// Create a set from an inclusive range `[from, to]`.
    ///
    /// # Panics
    /// Panics if `from > to`.
    pub fn range(from: usize, to: usize) -> Self {
        assert!(from <= to, "from ({from}) must be <= to ({to})");
        Self { cpus: (from..=to).collect() }
    }

    /// The CPU indices in this set.
    pub fn cpus(&self) -> &[usize] {
        &self.cpus
    }

    /// Number of CPUs in the set.
    pub fn len(&self) -> usize {
        self.cpus.len()
    }

    /// Returns `true` if the set contains no CPUs.
    pub fn is_empty(&self) -> bool {
        self.cpus.is_empty()
    }
}

// ── Linux ──────────────────────────────────────────────────────────────────

/// Pin the current thread to the given CPU set.
///
/// Calls `sched_setaffinity(0, ...)` — `0` means the calling thread.
/// Requires appropriate permissions (typically no special privilege needed
/// for same-process threads).
#[cfg(target_os = "linux")]
pub fn pin_thread(set: &CpuSet) -> io::Result<()> {
    unsafe {
        let mut cpu_set = std::mem::zeroed::<libc::cpu_set_t>();
        libc::CPU_ZERO(&mut cpu_set);
        for &cpu in set.cpus() {
            libc::CPU_SET(cpu, &mut cpu_set);
        }
        let ret = libc::sched_setaffinity(
            0,
            std::mem::size_of::<libc::cpu_set_t>(),
            &cpu_set,
        );
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Read the CPU affinity mask of the current thread.
///
/// Returns the set of CPUs the thread is currently allowed to run on.
#[cfg(target_os = "linux")]
pub fn get_affinity() -> io::Result<CpuSet> {
    let mut cpu_set = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    let ret = unsafe {
        libc::sched_getaffinity(
            0,
            std::mem::size_of::<libc::cpu_set_t>(),
            &mut cpu_set,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut cpus = Vec::new();
    for i in 0..libc::CPU_SETSIZE as usize {
        if unsafe { libc::CPU_ISSET(i, &cpu_set) } {
            cpus.push(i);
        }
    }
    Ok(CpuSet { cpus })
}

// ── Non-Linux stubs ────────────────────────────────────────────────────────

/// Pin the current thread to the given CPU set. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn pin_thread(set: &CpuSet) -> io::Result<()> {
    let _ = set;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "CPU affinity requires Linux",
    ))
}

/// Read the CPU affinity mask of the current thread. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn get_affinity() -> io::Result<CpuSet> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "CPU affinity requires Linux",
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_set_single() {
        let s = CpuSet::single(3);
        assert_eq!(s.cpus(), &[3]);
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
    }

    #[test]
    fn cpu_set_range() {
        let s = CpuSet::range(0, 3);
        assert_eq!(s.cpus(), &[0, 1, 2, 3]);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn cpu_set_is_empty() {
        let s = CpuSet { cpus: vec![] };
        assert!(s.is_empty());
    }

    #[test]
    #[should_panic]
    fn cpu_set_range_invalid() {
        let _ = CpuSet::range(5, 2);
    }

    /// On Linux: the affinity mask must contain at least one CPU.
    #[cfg(target_os = "linux")]
    #[test]
    fn get_affinity_returns_nonempty() {
        let set = get_affinity().expect("sched_getaffinity should succeed");
        assert!(!set.is_empty(), "at least one CPU must be in the affinity mask");
    }

    /// On non-Linux: pin_thread and get_affinity return Unsupported.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn stubs_return_unsupported() {
        let r = pin_thread(&CpuSet::single(0));
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Unsupported);
        let r = get_affinity();
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
