use std::io;

/// Snapshot of kernel tuning state relevant for HFT workloads.
///
/// Built by [`run()`] from `/proc` and `/sys` entries. All fields are
/// best-effort: a missing sysfs file results in a sensible default (0,
/// empty Vec, or "unknown") rather than a hard error.
#[derive(Debug, Clone)]
pub struct SystemReport {
    /// Number of 2 MB hugepages pre-allocated (`nr_hugepages`).
    /// 0 means none reserved — TLB pressure is high on large mmap regions.
    pub nr_hugepages_2mb: u64,

    /// Transparent hugepage mode: `"always"`, `"madvise"`, or `"never"`.
    /// `"madvise"` is recommended for Ashta: opt-in per-mapping via
    /// `madvise(MADV_HUGEPAGE)`.
    pub transparent_hugepages: String,

    /// CPUs listed in the `isolcpus` kernel parameter (parsed from sysfs).
    /// These cores are excluded from the normal scheduler — ideal for pinning
    /// the hot I/O and ingest threads.
    pub isolated_cpus: Vec<usize>,

    /// CPUs running without the periodic timer tick (`nohz_full`).
    /// Combined with `isolcpus`, these cores have minimal kernel interference.
    pub nohz_full_cpus: Vec<usize>,

    /// Per-CPU frequency governor, e.g., `(0, "performance")`.
    /// `"powersave"` introduces variable CPU frequency → variable latency.
    pub cpu_governors: Vec<(usize, String)>,

    /// Value of `/proc/sys/kernel/perf_event_paranoid`.
    /// -1 = full access (needed for `perf stat`), 3 = most restrictive.
    pub perf_paranoid: i32,
}

impl SystemReport {
    /// Format the report as a human-readable checklist.
    ///
    /// Each line is prefixed with `OK` or `!!` to highlight tuning gaps.
    pub fn display(&self) -> String {
        let mut out = String::from("=== Ashta-Kernel System Report ===\n");

        // Hugepages
        let hp_ok = self.nr_hugepages_2mb > 0;
        out.push_str(&format!(
            "  [{}] Hugepages 2MB       : {} pages ({} MiB)\n",
            if hp_ok { "OK" } else { "!!" },
            self.nr_hugepages_2mb,
            self.nr_hugepages_2mb * 2,
        ));

        // Transparent hugepages
        let thp_ok = self.transparent_hugepages == "madvise";
        out.push_str(&format!(
            "  [{}] Transparent HP      : {} (recommended: madvise)\n",
            if thp_ok { "OK" } else { "!!" },
            self.transparent_hugepages,
        ));

        // Isolated CPUs
        let iso_ok = !self.isolated_cpus.is_empty();
        out.push_str(&format!(
            "  [{}] Isolated CPUs       : {:?}\n",
            if iso_ok { "OK" } else { "!!" },
            self.isolated_cpus,
        ));

        // nohz_full
        let nohz_ok = !self.nohz_full_cpus.is_empty();
        out.push_str(&format!(
            "  [{}] nohz_full CPUs      : {:?}\n",
            if nohz_ok { "OK" } else { "!!" },
            self.nohz_full_cpus,
        ));

        // perf_paranoid
        let perf_ok = self.perf_paranoid <= 0;
        out.push_str(&format!(
            "  [{}] perf_event_paranoid : {}\n",
            if perf_ok { "OK" } else { "!!" },
            self.perf_paranoid,
        ));

        // CPU governors
        for (cpu, gov) in &self.cpu_governors {
            let gov_ok = gov == "performance";
            out.push_str(&format!(
                "  [{}] CPU {} governor     : {}\n",
                if gov_ok { "OK" } else { "!!" },
                cpu,
                gov,
            ));
        }

        out
    }
}

// ── Linux ──────────────────────────────────────────────────────────────────

/// Run a full HFT readiness check.
///
/// Reads from `/proc` and `/sys`. All fields are best-effort — missing
/// files degrade gracefully to zero / empty / "unknown".
#[cfg(target_os = "linux")]
pub fn run() -> io::Result<SystemReport> {
    // 1. Hugepages 2 MB
    let nr_hugepages_2mb = read_u64(
        "/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages",
    )
    .unwrap_or(0);

    // 2. Transparent hugepages — active mode is wrapped in brackets: "[madvise]"
    let thp_raw = std::fs::read_to_string(
        "/sys/kernel/mm/transparent_hugepage/enabled",
    )
    .unwrap_or_default();
    let transparent_hugepages = thp_raw
        .split_whitespace()
        .find(|s| s.starts_with('[') && s.ends_with(']'))
        .map(|s| s.trim_matches(['[', ']']).to_string())
        .unwrap_or_else(|| thp_raw.trim().to_string());

    // 3. Isolated CPUs
    let isolated_cpus = std::fs::read_to_string("/sys/devices/system/cpu/isolated")
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default();

    // 4. nohz_full
    let nohz_full_cpus = std::fs::read_to_string("/sys/devices/system/cpu/nohz_full")
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default();

    // 5. Per-CPU frequency governor
    let n_cpus = online_cpu_count();
    let mut cpu_governors = Vec::with_capacity(n_cpus);
    for i in 0..n_cpus {
        let path = format!(
            "/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_governor"
        );
        if let Ok(gov) = std::fs::read_to_string(&path) {
            cpu_governors.push((i, gov.trim().to_string()));
        }
    }

    // 6. perf_event_paranoid
    let perf_paranoid = read_i32("/proc/sys/kernel/perf_event_paranoid").unwrap_or(3);

    Ok(SystemReport {
        nr_hugepages_2mb,
        transparent_hugepages,
        isolated_cpus,
        nohz_full_cpus,
        cpu_governors,
        perf_paranoid,
    })
}

/// Parse a CPU list such as `"0-3,5,7-8"` into a `Vec<usize>`.
///
/// Used for `isolcpus`, `nohz_full`, and similar kernel parameters.
/// Empty or whitespace-only input returns an empty Vec.
pub fn parse_cpu_list(s: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in s.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                cpus.extend(a..=b);
            }
        } else if let Ok(n) = part.parse::<usize>() {
            cpus.push(n);
        }
    }
    cpus
}

#[cfg(target_os = "linux")]
fn read_u64(path: &str) -> io::Result<u64> {
    std::fs::read_to_string(path)?
        .trim()
        .parse::<u64>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(target_os = "linux")]
fn read_i32(path: &str) -> io::Result<i32> {
    std::fs::read_to_string(path)?
        .trim()
        .parse::<i32>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(target_os = "linux")]
fn online_cpu_count() -> usize {
    std::fs::read_to_string("/sys/devices/system/cpu/online")
        .map(|s| parse_cpu_list(&s).into_iter().max().map(|m| m + 1).unwrap_or(1))
        .unwrap_or(1)
}

// ── Non-Linux stub ─────────────────────────────────────────────────────────

/// Run a full HFT readiness check. Linux only.
#[cfg(not(target_os = "linux"))]
pub fn run() -> io::Result<SystemReport> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Kernel diagnostics require Linux",
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // parse_cpu_list is pure — test on all platforms.

    #[test]
    fn parse_range() {
        assert_eq!(parse_cpu_list("0-3"), vec![0, 1, 2, 3]);
    }

    #[test]
    fn parse_mixed() {
        assert_eq!(parse_cpu_list("0-1,3,5-6"), vec![0, 1, 3, 5, 6]);
    }

    #[test]
    fn parse_single() {
        assert_eq!(parse_cpu_list("7"), vec![7]);
    }

    #[test]
    fn parse_empty() {
        assert!(parse_cpu_list("").is_empty());
        assert!(parse_cpu_list("  ").is_empty());
    }

    #[test]
    fn display_marks_unconfigured_as_warning() {
        let report = SystemReport {
            nr_hugepages_2mb: 0,
            transparent_hugepages: "always".to_string(),
            isolated_cpus: vec![],
            nohz_full_cpus: vec![],
            cpu_governors: vec![(0, "powersave".to_string())],
            perf_paranoid: 3,
        };
        let s = report.display();
        // All fields untuned → should contain "!!" warnings
        assert!(s.contains("!!"));
        assert!(s.contains("0 pages"));
    }

    #[test]
    fn display_marks_tuned_as_ok() {
        let report = SystemReport {
            nr_hugepages_2mb: 512,
            transparent_hugepages: "madvise".to_string(),
            isolated_cpus: vec![2, 3],
            nohz_full_cpus: vec![2, 3],
            cpu_governors: vec![(0, "performance".to_string())],
            perf_paranoid: -1,
        };
        let s = report.display();
        assert!(s.contains("[OK]"));
        assert!(s.contains("512 pages"));
    }

    /// On Linux: diagnostics must complete without error.
    #[cfg(target_os = "linux")]
    #[test]
    fn run_succeeds_on_linux() {
        let report = run().expect("diagnostics should succeed on Linux");
        // perf_paranoid is always a valid int
        assert!(report.perf_paranoid >= -1);
    }

    /// On non-Linux: run() returns Unsupported.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn run_returns_unsupported() {
        assert_eq!(run().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
