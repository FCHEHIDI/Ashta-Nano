use std::io;

/// Hugepage statistics for a given page size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HugepageStatus {
    /// Page size in kilobytes (e.g., 2048 for 2 MB pages).
    pub page_size_kb: u64,
    /// Total number of pre-allocated hugepages.
    pub total: u64,
    /// Number of hugepages not yet mapped into any process.
    pub free: u64,
    /// `total - free`.
    pub used: u64,
}

// ── Linux ──────────────────────────────────────────────────────────────────

/// Read 2 MB hugepage statistics from sysfs.
///
/// Reads `/sys/kernel/mm/hugepages/hugepages-2048kB/{nr,free}_hugepages`.
///
/// A `total` of 0 means hugepages are not pre-allocated — the kernel has
/// not reserved any 2 MB pages. They can still be allocated on-demand via
/// `madvise(MADV_HUGEPAGE)` if transparent hugepages are set to `madvise`
/// or `always`.
#[cfg(target_os = "linux")]
pub fn status_2mb() -> io::Result<HugepageStatus> {
    status(2048)
}

/// Read hugepage statistics for the given `page_size_kb` from sysfs.
///
/// Common values: `2048` (2 MB), `1048576` (1 GB).
#[cfg(target_os = "linux")]
pub fn status(page_size_kb: u64) -> io::Result<HugepageStatus> {
    let base = format!(
        "/sys/kernel/mm/hugepages/hugepages-{}kB",
        page_size_kb
    );
    let total = read_sysfs_u64(&format!("{base}/nr_hugepages"))?;
    let free = read_sysfs_u64(&format!("{base}/free_hugepages"))?;
    Ok(HugepageStatus {
        page_size_kb,
        total,
        free,
        used: total.saturating_sub(free),
    })
}

#[cfg(target_os = "linux")]
fn read_sysfs_u64(path: &str) -> io::Result<u64> {
    std::fs::read_to_string(path)?
        .trim()
        .parse::<u64>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ── Non-Linux stubs ────────────────────────────────────────────────────────

/// Read 2 MB hugepage statistics. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn status_2mb() -> io::Result<HugepageStatus> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Hugepage status requires Linux",
    ))
}

/// Read hugepage statistics for the given page size. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn status(page_size_kb: u64) -> io::Result<HugepageStatus> {
    let _ = page_size_kb;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Hugepage status requires Linux",
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hugepage_status_fields_consistent() {
        // Construct a status and verify the invariant: used = total - free
        let s = HugepageStatus {
            page_size_kb: 2048,
            total: 128,
            free: 32,
            used: 96,
        };
        assert_eq!(s.used, s.total - s.free);
    }

    /// On Linux: sysfs must be readable (total may be 0 if not configured).
    #[cfg(target_os = "linux")]
    #[test]
    fn status_2mb_is_readable() {
        let result = status_2mb();
        assert!(result.is_ok(), "sysfs hugepages-2048kB should be readable: {result:?}");
        let s = result.unwrap();
        assert_eq!(s.page_size_kb, 2048);
        assert_eq!(s.used, s.total.saturating_sub(s.free));
    }

    /// On non-Linux: both functions return Unsupported.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn stubs_return_unsupported() {
        assert_eq!(status_2mb().unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(status(2048).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
