use std::time::Instant;

use ashta_core::{Event, EventKind, SymbolId};
use ashta_log::LogWriter;
use ashta_query::QueryEngine;

/// Nombre d'événements synthétiques générés pour la démo.
const EVENT_COUNT: usize = 10_000;

/// Symbole fictif — encodé sur 8 bytes, padé \0.
const SYMBOL: &str = "BTCUSDT";

/// Génère un flux d'événements synthétiques Trade sur `BTCUSDT`.
///
/// Args:
///     n: Nombre d'événements à générer.
///     base_price: Prix initial (en USD).
///
/// Returns:
///     Vec d'`Event` avec timestamps séquentiels à la microseconde.
fn generate_events(n: usize, base_price: f64) -> Vec<Event> {
    let symbol = SymbolId::from(SYMBOL);
    (0..n)
        .map(|i| Event {
            // timestamps espacés de 1 ms (1_000_000 ns) à partir de l'époque 0
            timestamp_ns: i as u64 * 1_000_000,
            symbol,
            price: base_price + (i as f64 * 0.01),
            volume: 0.1 + (i % 100) as f64 * 0.001,
            kind: EventKind::Trade,
            _pad: [0; 7],
        })
        .collect()
}

fn main() -> std::io::Result<()> {
    // ── 1. Répertoire temporaire ──────────────────────────────────────────
    let dir = std::env::temp_dir().join("ashta_demo_run");
    // Repart toujours d'un état propre
    let _ = std::fs::remove_dir_all(&dir);

    println!("=== Ashta-TS — end-to-end demo ===\n");
    println!("Log dir : {}", dir.display());
    println!("Events  : {EVENT_COUNT} x {SYMBOL} Trade\n");

    // ── 2. Génération des événements synthétiques ─────────────────────────
    let events = generate_events(EVENT_COUNT, 30_000.0);

    // ── 3. Écriture via LogWriter ─────────────────────────────────────────
    let mut writer = LogWriter::open(&dir)?;

    let t_write_start = Instant::now();
    for event in &events {
        writer.append(event)?;
    }
    let write_elapsed = t_write_start.elapsed();

    println!(
        "[WRITE] {} events en {:.2?}  ({:.0} ns/event)",
        EVENT_COUNT,
        write_elapsed,
        write_elapsed.as_nanos() as f64 / EVENT_COUNT as f64,
    );

    // ── 4. Seal + flush de l'index ────────────────────────────────────────
    // rotate() scelle le segment actif et flush index.bin sur disque.
    // Indispensable avant d'ouvrir QueryEngine (qui lit index.bin).
    writer.rotate()?;

    // ── 5. Requête via QueryEngine ────────────────────────────────────────
    let engine = QueryEngine::open(&dir)?;

    let symbol = SymbolId::from(SYMBOL);
    let t_start = 0u64;
    let t_end = EVENT_COUNT as u64 * 1_000_000; // fenêtre couvrant tout le log

    let t_query_start = Instant::now();
    let results = engine.read_range(symbol, t_start, t_end)?;
    let query_elapsed = t_query_start.elapsed();

    println!(
        "[QUERY] {} events lus en {:.2?}  ({:.0} ns/event)",
        results.len(),
        query_elapsed,
        if results.is_empty() {
            0.0
        } else {
            query_elapsed.as_nanos() as f64 / results.len() as f64
        },
    );

    // ── 6. Vérification de cohérence ─────────────────────────────────────
    let ok = results.len() == EVENT_COUNT;
    println!(
        "\n[CHECK] Écrits={}, Relus={} — {}",
        EVENT_COUNT,
        results.len(),
        if ok { "OK ✓" } else { "MISMATCH ✗" }
    );

    if !results.is_empty() {
        let first = &results[0];
        let last = &results[results.len() - 1];
        println!(
            "        Premier event : ts={}ns  price={:.2}",
            first.timestamp_ns, first.price
        );
        println!(
            "        Dernier event : ts={}ns  price={:.2}",
            last.timestamp_ns, last.price
        );
    }

    // ── 7. Nettoyage ──────────────────────────────────────────────────────
    let _ = std::fs::remove_dir_all(&dir);

    if ok {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "incohérence : {} écrits vs {} relus",
                EVENT_COUNT,
                results.len()
            ),
        ))
    }
}
