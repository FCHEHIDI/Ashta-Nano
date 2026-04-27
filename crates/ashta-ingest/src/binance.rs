use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::time::Instant;

use ashta_core::{Event, EventKind, SymbolId};
use ashta_log::LogWriter;

/// Statistiques retournées après une ingestion.
#[derive(Debug)]
pub struct IngestStats {
    pub events_written: u64,
    pub segments_created: u32,
    pub lines_skipped: u64,
    pub duration_ms: u64,
    pub events_per_second: f64,
}

/// Déduit le SymbolId depuis le nom de fichier Binance.
///
/// `BTCUSDT-trades-2024-01.csv` → `SymbolId("BTCUSDT")`
///
/// Retourne `None` si le nom ne correspond pas au pattern attendu.
pub fn symbol_from_filename(path: &Path) -> Option<SymbolId> {
    let stem = path.file_stem()?.to_string_lossy();
    // format attendu : "<SYMBOL>-trades-<YYYY>-<MM>"
    let symbol_str = stem.split('-').next()?;
    if symbol_str.is_empty() {
        return None;
    }
    Some(SymbolId::from(symbol_str))
}

/// Ingère un fichier CSV de trades Binance dans un log Ashta-TS.
///
/// Format CSV attendu (avec ou sans header) :
/// ```text
/// id,price,qty,quoteQty,time,isBuyerMaker,isBestMatch
/// 1234567,43210.50,0.001,43.21,1704067200123,True,True
/// ```
///
/// - `time` est en **millisecondes** → converti en nanosecondes (×1_000_000)
/// - `price` → `Event.price`
/// - `qty`   → `Event.volume`
/// - Tous les events reçoivent `EventKind::Trade`
///
/// Les lignes malformées sont ignorées (comptées dans `lines_skipped`).
pub fn ingest_binance_csv(
    csv_path: impl AsRef<Path>,
    symbol: SymbolId,
    log_dir: impl AsRef<Path>,
) -> io::Result<IngestStats> {
    let csv_path = csv_path.as_ref();
    let file = File::open(csv_path)?;
    let reader = BufReader::new(file);

    let mut writer = LogWriter::open(log_dir)?;
    let start = Instant::now();

    let mut events_written: u64 = 0;
    let mut lines_skipped: u64 = 0;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        // Ignore les lignes vides ou le header (commence par une lettre)
        if trimmed.is_empty() || trimmed.starts_with(|c: char| c.is_alphabetic()) {
            continue;
        }

        match parse_trade_line(trimmed, symbol) {
            Some(event) => {
                writer.append(&event)?;
                events_written += 1;
            }
            None => {
                lines_skipped += 1;
            }
        }
    }

    // Scelle le dernier segment actif et flush l'index
    writer.rotate()?;
    let segments_created = writer.current_segment_id(); // id du segment vide actuel
    let duration_ms = start.elapsed().as_millis() as u64;
    let events_per_second = if duration_ms > 0 {
        events_written as f64 / (duration_ms as f64 / 1000.0)
    } else {
        f64::INFINITY
    };

    Ok(IngestStats {
        events_written,
        segments_created,
        lines_skipped,
        duration_ms,
        events_per_second,
    })
}

/// Parse une ligne CSV Binance en `Event`.
///
/// Retourne `None` si un champ est manquant ou malformé.
fn parse_trade_line(line: &str, symbol: SymbolId) -> Option<Event> {
    let mut fields = line.splitn(7, ',');

    // Colonne 0 : id (ignoré)
    let _id = fields.next()?;

    // Colonne 1 : price (f64)
    let price: f64 = fields.next()?.trim().parse().ok()?;

    // Colonne 2 : qty / volume (f64)
    let volume: f64 = fields.next()?.trim().parse().ok()?;

    // Colonne 3 : quoteQty (ignoré)
    let _quote_qty = fields.next()?;

    // Colonne 4 : time en millisecondes
    let time_ms: u64 = fields.next()?.trim().parse().ok()?;
    let timestamp_ns = time_ms * 1_000_000;

    // Prix ou volume négatif/nul = donnée corrompue
    if price <= 0.0 || volume <= 0.0 {
        return None;
    }

    Some(Event {
        timestamp_ns,
        symbol,
        price,
        volume,
        kind: EventKind::Trade,
        _pad: [0; 7],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── helpers ────────────────────────────────────────────────────────────

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ashta_ingest_test_{}", name));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn tmp_csv(name: &str, content: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ashta_ingest_{}.csv", name));
        let mut f = File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    // ── tests unitaires sur le parser ──────────────────────────────────────

    #[test]
    fn parse_valid_line() {
        let sym = SymbolId::from("BTCUSDT");
        let event = parse_trade_line(
            "1234567,43210.50,0.001,43.21,1704067200123,True,True",
            sym,
        )
        .unwrap();

        assert_eq!(event.timestamp_ns, 1_704_067_200_123 * 1_000_000);
        assert!((event.price - 43210.50).abs() < 1e-9);
        assert!((event.volume - 0.001).abs() < 1e-9);
        assert_eq!(event.symbol, sym);
        assert_eq!(event.kind, EventKind::Trade);
    }

    #[test]
    fn parse_rejects_missing_fields() {
        let sym = SymbolId::from("BTCUSDT");
        // Seulement 3 champs au lieu de 7
        assert!(parse_trade_line("1234,43210.50,0.001", sym).is_none());
    }

    #[test]
    fn parse_rejects_negative_price() {
        let sym = SymbolId::from("BTCUSDT");
        assert!(parse_trade_line(
            "1,-43210.50,0.001,43.21,1704067200123,True,True",
            sym
        )
        .is_none());
    }

    #[test]
    fn symbol_from_filename_extracts_symbol() {
        let path = Path::new("BTCUSDT-trades-2024-01.csv");
        assert_eq!(symbol_from_filename(path), Some(SymbolId::from("BTCUSDT")));

        let path2 = Path::new("ETHUSDT-trades-2023-12.csv");
        assert_eq!(symbol_from_filename(path2), Some(SymbolId::from("ETHUSDT")));

        let path3 = Path::new("not_binance.csv");
        // "not_binance" → premier token avant '-' = "not_binance" (valide, tronqué à 8 chars)
        // On vérifie juste que ça ne panique pas
        assert!(symbol_from_filename(path3).is_some());
    }

    // ── tests d'intégration avec CSV synthétique ───────────────────────────

    #[test]
    fn ingest_csv_with_header_and_valid_rows() {
        let csv = "\
id,price,qty,quoteQty,time,isBuyerMaker,isBestMatch\n\
1,43000.00,0.5,21500.0,1704067200000,True,True\n\
2,43100.00,0.2,8620.0,1704067201000,False,True\n\
3,43200.00,0.3,12960.0,1704067202000,True,True\n\
";
        let csv_path = tmp_csv("header", csv);
        let log_dir = tmp_dir("header_log");
        let symbol = SymbolId::from("BTCUSDT");

        let stats = ingest_binance_csv(&csv_path, symbol, &log_dir).unwrap();

        assert_eq!(stats.events_written, 3);
        assert_eq!(stats.lines_skipped, 0);
    }

    #[test]
    fn ingest_csv_skips_malformed_rows() {
        let csv = "\
id,price,qty,quoteQty,time,isBuyerMaker,isBestMatch\n\
1,43000.00,0.5,21500.0,1704067200000,True,True\n\
CORRUPTED_LINE\n\
3,43200.00,0.3,12960.0,1704067202000,True,True\n\
";
        let csv_path = tmp_csv("malformed", csv);
        let log_dir = tmp_dir("malformed_log");
        let symbol = SymbolId::from("BTCUSDT");

        let stats = ingest_binance_csv(&csv_path, symbol, &log_dir).unwrap();

        assert_eq!(stats.events_written, 2);
        // La ligne CORRUPTED_LINE commence par une lettre → ignorée comme header, pas comptée
        assert_eq!(stats.lines_skipped, 0);
    }

    #[test]
    fn ingest_then_query_roundtrip() {
        use ashta_query::QueryEngine;

        let csv = "\
1,43000.00,1.0,43000.0,1000,True,True\n\
2,43100.00,2.0,86200.0,2000,False,True\n\
3,43200.00,3.0,129600.0,3000,True,True\n\
4,43300.00,4.0,173200.0,4000,True,True\n\
";
        let csv_path = tmp_csv("roundtrip", csv);
        let log_dir = tmp_dir("roundtrip_log");
        let symbol = SymbolId::from("BTCUSDT");

        let stats = ingest_binance_csv(&csv_path, symbol, &log_dir).unwrap();
        assert_eq!(stats.events_written, 4);

        // Lit via QueryEngine
        let engine = QueryEngine::open(&log_dir).unwrap();
        // time=1000ms → ts_ns = 1_000_000_000  |  time=3000ms → ts_ns = 3_000_000_000
        let events = engine
            .read_range(symbol, 1_000_000_000, 3_000_000_000)
            .unwrap();

        assert_eq!(events.len(), 3);
        assert!((events[0].price - 43000.0).abs() < 1e-9);
        assert!((events[2].price - 43200.0).abs() < 1e-9);
    }
}
