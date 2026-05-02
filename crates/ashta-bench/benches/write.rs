mod common;

use common::{make_events, temp_dir};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ashta_log::LogWriter;

/// Mesure le throughput d'écriture brut de `LogWriter::append`.
///
/// Trois tailles : 1k / 10k / 100k events.
/// Throughput affiché en events/s et bytes/s (40B par event).
fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_throughput");

    for n in [1_000usize, 10_000, 100_000] {
        let events = make_events(n, "BTCUSDT");

        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(BenchmarkId::from_parameter(n), &events, |b, events| {
            b.iter(|| {
                let dir = temp_dir(&format!("write_{}", n));
                let mut writer = LogWriter::open(&dir).unwrap();
                for e in events {
                    writer.append(e).unwrap();
                }
                writer.rotate().unwrap();
            });
        });
    }

    group.finish();
}

/// Mesure uniquement le `append` sans rotate, pour isoler le hot path.
fn bench_append_hot(c: &mut Criterion) {
    let events = make_events(10_000, "BTCUSDT");
    let dir = temp_dir("append_hot");
    let mut writer = LogWriter::open(&dir).unwrap();

    // Préchauffage : on réutilise le même writer pour que le segment
    // soit déjà ouvert — on mesure l'appel append pur.
    c.bench_function("append_single_event", |b| {
        b.iter(|| {
            writer.append(&events[0]).unwrap();
        });
    });
}

criterion_group!(benches, bench_write, bench_append_hot);
criterion_main!(benches);
