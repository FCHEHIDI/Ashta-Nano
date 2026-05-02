mod common;

use common::{make_events, temp_dir};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ashta_log::{LogWriter, SegmentReader};

/// Prépare un répertoire de log scellé avec `n` events.
/// Retourne le path du premier segment.
fn setup_sealed_segment(n: usize) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = temp_dir(&format!("read_{}", n));
    let events = make_events(n, "BTCUSDT");

    let mut writer = LogWriter::open(&dir).unwrap();
    for e in &events {
        writer.append(e).unwrap();
    }
    writer.rotate().unwrap();

    let seg_path = dir.join("segment_0000.alog");
    (dir, seg_path)
}

/// Mesure la vitesse d'itération complète d'un `SegmentReader` (mmap).
///
/// Ce bench quantifie le coût de lecture séquentielle + décodage Event
/// depuis un mapping mémoire. Le fichier sera dans le page cache du kernel
/// après la première passe — on mesure la vitesse "cache chaud".
fn bench_read_sequential(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_sequential");

    for n in [1_000usize, 10_000, 100_000] {
        let (_, seg_path) = setup_sealed_segment(n);

        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(BenchmarkId::from_parameter(n), &seg_path, |b, path| {
            b.iter(|| {
                let reader = SegmentReader::open(path).unwrap();
                // Consomme l'itérateur entier — force la lecture de chaque Event
                let count = reader.count();
                assert_eq!(count, n);
            });
        });
    }

    group.finish();
}

/// Mesure le coût d'ouverture seule du `SegmentReader` (mmap syscall).
fn bench_mmap_open(c: &mut Criterion) {
    let (_, seg_path) = setup_sealed_segment(10_000);

    c.bench_function("mmap_open_10k", |b| {
        b.iter(|| {
            let _ = SegmentReader::open(&seg_path).unwrap();
        });
    });
}

criterion_group!(benches, bench_read_sequential, bench_mmap_open);
criterion_main!(benches);
