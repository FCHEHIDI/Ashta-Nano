//! `ashta-kernel` — Linux kernel tuning API for HFT workloads.
//!
//! Exposes low-level OS controls that reduce latency jitter on a Linux host:
//!
//! | Module          | What it controls                                 |
//! |-----------------|--------------------------------------------------|
//! | [`cpu`]         | Thread CPU affinity — `sched_setaffinity(2)`     |
//! | [`hugepages`]   | Hugepage status — reads `/sys/kernel/mm/`        |
//! | [`sched`]       | Real-time scheduler — `SCHED_FIFO` / `SCHED_RR` |
//! | [`diagnostics`] | Full HFT readiness report from `/proc` + `/sys`  |
//!
//! All public functions are **Linux-only**: on other platforms they compile
//! but return `Err(io::ErrorKind::Unsupported)`.

pub mod cpu;
pub mod diagnostics;
pub mod hugepages;
pub mod sched;
