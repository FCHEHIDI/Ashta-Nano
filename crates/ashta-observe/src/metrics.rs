use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Nombre de buckets de l'histogramme.
/// 64 buckets couvrent 1 ns à 2^63 ns (~292 ans) — les buckets 10-30 sont
/// les plus actifs en HFT (1 µs à 1 s).
const NUM_BUCKETS: usize = 64;

// ─── Metrics ──────────────────────────────────────────────────────────────────

/// Point de collecte central pour les métriques d'Ashta-TS.
///
/// Conçu pour être partagé via `Arc<Metrics>` entre threads.
/// Tous les compteurs sont `AtomicU64` — zéro verrou, zéro allocation sur le hot path.
///
/// # Utilisation
///
/// ```rust
/// use std::sync::Arc;
/// use ashta_observe::Metrics;
///
/// let m = Arc::new(Metrics::new());
/// m.events_written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
/// let snap = m.snapshot();
/// println!("{}", snap);
/// ```
pub struct Metrics {
    /// Total d'events écrits dans le log depuis le démarrage.
    pub events_written: AtomicU64,
    /// Total d'events lus (replay + query confondus).
    pub events_read: AtomicU64,
    /// Total de bytes écrits sur disque (données brutes, hors index).
    pub bytes_written: AtomicU64,
    /// Nombre de segments scellés (rotation complète, fsync inclus).
    pub segments_sealed: AtomicU64,
    /// Histogramme de latence pour `write_event` (nanosecondes par appel).
    pub write_latency: LatencyHistogram,
    /// Histogramme de latence pour `read_range` / replay (nanosecondes par appel).
    pub read_latency: LatencyHistogram,
}

impl Metrics {
    /// Crée une instance avec tous les compteurs à zéro.
    pub fn new() -> Self {
        Self {
            events_written: AtomicU64::new(0),
            events_read: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            segments_sealed: AtomicU64::new(0),
            write_latency: LatencyHistogram::new(),
            read_latency: LatencyHistogram::new(),
        }
    }

    /// Retourne un snapshot immutable de l'état courant.
    ///
    /// Chaque compteur est lu avec `Ordering::Relaxed` : le snapshot est
    /// cohérent par valeur individuelle, pas globalement transactionnel.
    /// Suffisant pour de l'observabilité — pas pour de la coordination.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_written: self.events_written.load(Ordering::Relaxed),
            events_read: self.events_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            segments_sealed: self.segments_sealed.load(Ordering::Relaxed),
            write_latency: self.write_latency.snapshot(),
            read_latency: self.read_latency.snapshot(),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

// ─── LatencyHistogram ─────────────────────────────────────────────────────────

/// Histogramme de latence lock-free à buckets puissance-de-2.
///
/// Bucket `k` accumule les mesures dont le bit le plus significatif est en position `k`.
/// Concrètement :
/// - bucket  0 : 0 ns
/// - bucket  1 : [1, 2) ns
/// - bucket 10 : [512, 1024) ns   ≈ sous-microseconde
/// - bucket 20 : [524µs, 1ms)
/// - bucket 30 : [537ms, 1.07s)
/// - bucket 63 : ≥ 2^62 ns        (overflow — ne devrait jamais arriver en HFT)
///
/// Toutes les opérations sont lock-free (`AtomicU64::fetch_add`, `Ordering::Relaxed`).
/// Zéro allocation — les buckets vivent sur la pile ou dans le `Metrics` propriétaire.
pub struct LatencyHistogram {
    buckets: [AtomicU64; NUM_BUCKETS],
}

impl LatencyHistogram {
    /// Crée un histogramme vide.
    pub fn new() -> Self {
        // SAFETY: AtomicU64 a le même layout que u64 ; la valeur zéro est
        // une représentation valide pour AtomicU64::new(0).
        Self {
            buckets: unsafe { std::mem::zeroed() },
        }
    }

    /// Enregistre une mesure en nanosecondes.
    ///
    /// `nanos = 0` va dans le bucket 0 (cas dégénéré, jamais en pratique).
    /// Opération O(1), lock-free — adaptée au hot path d'ingestion.
    #[inline]
    pub fn record(&self, nanos: u64) {
        let idx = Self::bucket_index(nanos);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Retourne l'index du bucket pour `nanos`.
    ///
    /// Algorithme : position du bit le plus significatif = 64 - leading_zeros.
    /// Exemple : nanos=1 → lz=63 → idx=1 ; nanos=1023 → lz=54 → idx=10.
    #[inline]
    fn bucket_index(nanos: u64) -> usize {
        if nanos == 0 {
            return 0;
        }
        let idx = 64 - nanos.leading_zeros() as usize;
        idx.min(NUM_BUCKETS - 1)
    }

    /// Retourne un snapshot immutable (copie des compteurs).
    pub fn snapshot(&self) -> HistogramSnapshot {
        let mut counts = [0u64; NUM_BUCKETS];
        for (i, bucket) in self.buckets.iter().enumerate() {
            counts[i] = bucket.load(Ordering::Relaxed);
        }
        HistogramSnapshot { counts }
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Probe ────────────────────────────────────────────────────────────────────

/// Timer RAII — mesure la durée d'un bloc et l'enregistre à la sortie.
///
/// La mesure est prise au `drop()` : elle est garantie même en cas de retour
/// anticipé ou de panique déroulante.
///
/// # Exemple
///
/// ```rust
/// use ashta_observe::{Metrics, Probe};
///
/// let m = Metrics::new();
/// {
///     let _p = Probe::write(&m);
///     // ... code à mesurer ...
/// } // ← nanoseconde enregistrée dans write_latency ici
/// ```
pub struct Probe<'a> {
    histogram: &'a LatencyHistogram,
    start: Instant,
}

impl<'a> Probe<'a> {
    /// Démarre un timer pour la latence d'écriture (`write_event`).
    #[inline]
    pub fn write(metrics: &'a Metrics) -> Self {
        Self {
            histogram: &metrics.write_latency,
            start: Instant::now(),
        }
    }

    /// Démarre un timer pour la latence de lecture (`read_range`, replay).
    #[inline]
    pub fn read(metrics: &'a Metrics) -> Self {
        Self {
            histogram: &metrics.read_latency,
            start: Instant::now(),
        }
    }
}

impl Drop for Probe<'_> {
    #[inline]
    fn drop(&mut self) {
        let nanos = self.start.elapsed().as_nanos() as u64;
        self.histogram.record(nanos);
    }
}

// ─── Snapshots ────────────────────────────────────────────────────────────────

/// Vue immutable des métriques à un instant T.
///
/// Obtenu via [`Metrics::snapshot`]. Peut être cloné, loggué, sérialisé.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub events_written: u64,
    pub events_read: u64,
    pub bytes_written: u64,
    pub segments_sealed: u64,
    pub write_latency: HistogramSnapshot,
    pub read_latency: HistogramSnapshot,
}

impl fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Ashta-TS Metrics ===")?;
        writeln!(f, "  events_written  : {}", self.events_written)?;
        writeln!(f, "  events_read     : {}", self.events_read)?;
        writeln!(
            f,
            "  bytes_written   : {} ({:.2} MiB)",
            self.bytes_written,
            self.bytes_written as f64 / (1024.0 * 1024.0)
        )?;
        writeln!(f, "  segments_sealed : {}", self.segments_sealed)?;
        writeln!(f, "  write_latency   : {}", self.write_latency)?;
        writeln!(f, "  read_latency    : {}", self.read_latency)?;
        Ok(())
    }
}

/// Vue immutable d'un histogramme de latence.
///
/// Fournit p50 / p90 / p99 en nanosecondes (borne supérieure du bucket).
/// Les percentiles sont approximatifs — précision d'un facteur 2 (résolution des buckets).
#[derive(Debug, Clone)]
pub struct HistogramSnapshot {
    counts: [u64; NUM_BUCKETS],
}

impl HistogramSnapshot {
    /// Nombre total de mesures enregistrées.
    pub fn count(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// Percentile `p` (0.0–1.0) en nanosecondes.
    ///
    /// Retourne la borne supérieure du bucket correspondant.
    /// Retourne 0 si aucune mesure.
    ///
    /// Exemples : `percentile(0.50)` → p50, `percentile(0.99)` → p99.
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.count();
        if total == 0 {
            return 0;
        }
        let target = ((total as f64 * p).ceil() as u64).max(1);
        let mut cumulative = 0u64;
        for (i, &count) in self.counts.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                // Borne supérieure du bucket i = 2^i (ou 1 pour bucket 0)
                return if i == 0 { 1 } else { 1u64 << i };
            }
        }
        u64::MAX
    }

    /// Médiane (p50) en nanosecondes.
    pub fn p50(&self) -> u64 {
        self.percentile(0.50)
    }

    /// p90 en nanosecondes.
    pub fn p90(&self) -> u64 {
        self.percentile(0.90)
    }

    /// p99 en nanosecondes.
    pub fn p99(&self) -> u64 {
        self.percentile(0.99)
    }
}

impl fmt::Display for HistogramSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "count={} p50={} p90={} p99={}",
            self.count(),
            fmt_ns(self.p50()),
            fmt_ns(self.p90()),
            fmt_ns(self.p99()),
        )
    }
}

/// Formate une durée en nanosecondes en unité lisible.
fn fmt_ns(nanos: u64) -> String {
    if nanos == 0 {
        return "0ns".to_string();
    }
    if nanos < 1_000 {
        return format!("{}ns", nanos);
    }
    if nanos < 1_000_000 {
        return format!("{:.1}µs", nanos as f64 / 1_000.0);
    }
    if nanos < 1_000_000_000 {
        return format!("{:.1}ms", nanos as f64 / 1_000_000.0);
    }
    format!("{:.2}s", nanos as f64 / 1_000_000_000.0)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── bucket_index ──────────────────────────────────────────────────────

    #[test]
    fn bucket_index_zero() {
        assert_eq!(LatencyHistogram::bucket_index(0), 0);
    }

    #[test]
    fn bucket_index_powers_of_two() {
        // nanos=1 → MSB à position 0 → idx=1
        assert_eq!(LatencyHistogram::bucket_index(1), 1);
        // nanos=2 → MSB à position 1 → idx=2
        assert_eq!(LatencyHistogram::bucket_index(2), 2);
        // nanos=3 → MSB à position 1 → idx=2
        assert_eq!(LatencyHistogram::bucket_index(3), 2);
        // nanos=4 → MSB à position 2 → idx=3
        assert_eq!(LatencyHistogram::bucket_index(4), 3);
        // 1 µs = 1000 ns → MSB à position 9 → idx=10
        assert_eq!(LatencyHistogram::bucket_index(1_000), 10);
        // 1 ms = 1_000_000 ns → idx=20
        assert_eq!(LatencyHistogram::bucket_index(1_000_000), 20);
    }

    #[test]
    fn bucket_index_never_out_of_bounds() {
        assert!(LatencyHistogram::bucket_index(u64::MAX) < NUM_BUCKETS);
    }

    // ── record + snapshot ─────────────────────────────────────────────────

    #[test]
    fn record_increments_correct_bucket() {
        let h = LatencyHistogram::new();
        h.record(1_000); // bucket 10
        h.record(1_000);
        h.record(500); // bucket 9

        let snap = h.snapshot();
        assert_eq!(snap.counts[10], 2);
        assert_eq!(snap.counts[9], 1);
        assert_eq!(snap.count(), 3);
    }

    #[test]
    fn empty_histogram_count_is_zero() {
        let h = LatencyHistogram::new();
        let snap = h.snapshot();
        assert_eq!(snap.count(), 0);
    }

    // ── percentiles ───────────────────────────────────────────────────────

    #[test]
    fn percentile_returns_zero_on_empty() {
        let h = LatencyHistogram::new();
        let snap = h.snapshot();
        assert_eq!(snap.p50(), 0);
        assert_eq!(snap.p99(), 0);
    }

    #[test]
    fn percentile_single_bucket() {
        let h = LatencyHistogram::new();
        // Enregistre 100 mesures de 1000 ns (bucket 10 → borne sup = 2^10 = 1024)
        for _ in 0..100 {
            h.record(1_000);
        }
        let snap = h.snapshot();
        assert_eq!(snap.p50(), 1024);
        assert_eq!(snap.p99(), 1024);
    }

    #[test]
    fn percentile_two_buckets() {
        let h = LatencyHistogram::new();
        // 90 mesures à 1000 ns (bucket 10), 10 mesures à 1_000_000 ns (bucket 20)
        for _ in 0..90 {
            h.record(1_000);
        }
        for _ in 0..10 {
            h.record(1_000_000);
        }
        let snap = h.snapshot();
        // p50 → 50e sample → bucket 10 (cumul=90 ≥ 50) → borne sup = 2^10 = 1024
        assert_eq!(snap.p50(), 1024);
        // p90 → 90e sample → bucket 10 (cumul=90 ≥ 90) → 1024
        assert_eq!(snap.p90(), 1024);
        // p99 → 99e sample → bucket 10 épuisé (cumul=90 < 99), bucket 20 (cumul=100 ≥ 99)
        // → borne sup = 2^20 = 1_048_576
        assert_eq!(snap.p99(), 1 << 20);
        // p100 → dernier sample → bucket 20
        assert_eq!(snap.percentile(1.0), 1 << 20);
    }

    // ── Probe RAII ────────────────────────────────────────────────────────

    #[test]
    fn probe_write_records_on_drop() {
        let m = Metrics::new();
        {
            let _p = Probe::write(&m);
            // Simule un "travail"
            std::hint::black_box(42u64);
        } // drop ici → enregistrement

        let snap = m.snapshot();
        assert_eq!(snap.write_latency.count(), 1);
        assert_eq!(snap.read_latency.count(), 0);
    }

    #[test]
    fn probe_read_records_on_drop() {
        let m = Metrics::new();
        {
            let _p = Probe::read(&m);
        }
        let snap = m.snapshot();
        assert_eq!(snap.read_latency.count(), 1);
        assert_eq!(snap.write_latency.count(), 0);
    }

    // ── Metrics compteurs ─────────────────────────────────────────────────

    #[test]
    fn metrics_counters_are_independent() {
        let m = Metrics::new();
        m.events_written.fetch_add(1_000, Ordering::Relaxed);
        m.bytes_written.fetch_add(40_000, Ordering::Relaxed);
        m.segments_sealed.fetch_add(1, Ordering::Relaxed);

        let snap = m.snapshot();
        assert_eq!(snap.events_written, 1_000);
        assert_eq!(snap.events_read, 0);
        assert_eq!(snap.bytes_written, 40_000);
        assert_eq!(snap.segments_sealed, 1);
    }

    // ── thread safety ─────────────────────────────────────────────────────

    #[test]
    fn metrics_shared_across_threads() {
        let m = Arc::new(Metrics::new());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let m = Arc::clone(&m);
            handles.push(std::thread::spawn(move || {
                for _ in 0..1_000 {
                    m.events_written.fetch_add(1, Ordering::Relaxed);
                    m.write_latency.record(500);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let snap = m.snapshot();
        assert_eq!(snap.events_written, 4_000);
        assert_eq!(snap.write_latency.count(), 4_000);
    }

    // ── Display ───────────────────────────────────────────────────────────

    #[test]
    fn metrics_snapshot_display_does_not_panic() {
        let m = Metrics::new();
        m.events_written.fetch_add(42, Ordering::Relaxed);
        m.write_latency.record(1_500);
        let snap = m.snapshot();
        let out = format!("{}", snap);
        assert!(out.contains("events_written"));
        assert!(out.contains("42"));
    }

    // ── fmt_ns ────────────────────────────────────────────────────────────

    #[test]
    fn fmt_ns_units() {
        assert_eq!(fmt_ns(0), "0ns");
        assert_eq!(fmt_ns(500), "500ns");
        assert_eq!(fmt_ns(1_500), "1.5µs");
        assert_eq!(fmt_ns(2_500_000), "2.5ms");
        assert_eq!(fmt_ns(1_000_000_000), "1.00s");
    }
}
