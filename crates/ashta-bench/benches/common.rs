/// Fixture partagée : génère N events séquentiels pour les benchmarks.
use ashta_core::{Event, EventKind, SymbolId};

pub fn make_events(n: usize, symbol: &str) -> Vec<Event> {
    let sym = SymbolId::from(symbol);
    (0..n)
        .map(|i| Event {
            timestamp_ns: 1_700_000_000_000_000_000u64 + i as u64 * 100,
            symbol: sym,
            price: 60_000.0 + i as f64 * 0.01,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        })
        .collect()
}

pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ashta_bench_{}", name));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
