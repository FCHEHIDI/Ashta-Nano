mod common;

use common::{make_events, temp_dir};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use ashta_log::LogWriter;
use ashta_query::QueryEngine;
use ashta_core::SymbolId;

/// Prépare un log multi-symboles avec plusieurs segments scellés.
/// Retourne le répertoire + la plage de timestamps.
fn setup_log(events_per_segment: usize, n_segments: usize) -> (std::path::PathBuf, u64, u64) {
    let dir = temp_dir(&format!("query_{}x{}", events_per_segment, n_segments));
    let total = events_per_segment * n_segments;
    let symbols = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];

    let mut writer = LogWriter::open(&dir).unwrap();

    for i in 0..total {
        use ashta_core::{Event, EventKind};
        let sym = symbols[i % symbols.len()];
        let e = Event {
            timestamp_ns: 1_700_000_000_000_000_000u64 + i as u64 * 1_000,
            symbol: SymbolId::from(sym),
            price: 60_000.0 + i as f64 * 0.01,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        };
        writer.append(&e).unwrap();

        // Force la rotation à chaque tranche pour créer les N segments
        if (i + 1) % events_per_segment == 0 {
            writer.rotate().unwrap();
        }
    }

    let t_start = 1_700_000_000_000_000_000u64;
    let t_end   = t_start + total as u64 * 1_000;
    (dir, t_start, t_end)
}

/// Mesure la latence de `read_range` sur 1 segment vs 4 segments.
///
/// Inclut : zone map pruning + ouverture mmap + scan + filtre.
fn bench_query_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_read_range");

    for n_segs in [1usize, 4] {
        let (dir, t_start, t_end) = setup_log(1_000, n_segs);
        let engine = QueryEngine::open(&dir).unwrap();
        let symbol = SymbolId::from("BTCUSDT");

        group.bench_with_input(
            BenchmarkId::new("segments", n_segs),
            &n_segs,
            |b, _| {
                b.iter(|| {
                    let results = engine.read_range(symbol, t_start, t_end).unwrap();
                    assert!(!results.is_empty());
                });
            },
        );
    }

    group.finish();
}

/// Mesure l'effet du pruning : requête sur une plage qui ne touche qu'1 segment
/// sur 4 → le zone map doit éliminer les 3 autres.
fn bench_query_pruning(c: &mut Criterion) {
    let (dir, t_start, _) = setup_log(1_000, 4);
    let engine = QueryEngine::open(&dir).unwrap();
    let symbol = SymbolId::from("BTCUSDT");

    // Plage restreinte au premier quart des timestamps
    let t_narrow_end = t_start + 1_000 * 1_000;

    c.bench_function("query_pruning_1_of_4_segments", |b| {
        b.iter(|| {
            let _ = engine.read_range(symbol, t_start, t_narrow_end).unwrap();
        });
    });
}

criterion_group!(benches, bench_query_range, bench_query_pruning);
criterion_main!(benches);
