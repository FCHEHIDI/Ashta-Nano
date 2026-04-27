use std::io;
use std::path::{Path, PathBuf};

use ashta_core::{Event, SymbolId};
use ashta_index::SegmentIndex;
use ashta_log::SegmentReader;

const SEGMENT_PREFIX: &str = "segment_";
const SEGMENT_EXT: &str = ".alog";

/// Moteur de requête en lecture seule sur un répertoire de log.
///
/// Ne possède ni verrou ni connexion ouverte : chaque appel à `read_range`
/// ouvre et ferme les fichiers nécessaires, sans état résiduel entre deux appels.
///
/// Invariant : le répertoire et l'index doivent être cohérents (écrits par `LogWriter`).
pub struct QueryEngine {
    dir: PathBuf,
    index: SegmentIndex,
}

impl QueryEngine {
    /// Ouvre le répertoire et charge l'index depuis `index.bin`.
    ///
    /// Peut être ouvert en même temps qu'un `LogWriter` en écriture,
    /// à condition de ne lire que des segments déjà scelés.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let index_path = dir.join("index.bin");
        let index = SegmentIndex::open(&index_path)?;
        Ok(Self { dir, index })
    }

    /// Retourne tous les events du symbole `symbol` dont le timestamp est
    /// dans l'intervalle **fermé** `[t_start, t_end]`.
    ///
    /// Algorithme :
    /// 1. Pruning via zone map : élimine les segments sans intersection temporelle.
    /// 2. Pour chaque segment candidat : scan séquentiel + filtre symbol + ts.
    /// 3. Résultat trié dans l'ordre d'écriture (ordre naturel des segments).
    ///
    /// Complexité : O(k × n_seg) où k = nombre de segments candidats, n_seg = events/segment.
    pub fn read_range(
        &self,
        symbol: SymbolId,
        t_start: u64,
        t_end: u64,
    ) -> io::Result<Vec<Event>> {
        // Étape 1 — pruning par l'index (zone map)
        let candidate_ids = self.index.query(symbol, t_start, t_end);

        let mut results = Vec::new();

        // Étape 2 — scan des segments candidats
        for seg_id in candidate_ids {
            let seg_path = self.segment_path(seg_id);

            // Le segment actif (non scellé) n'a pas encore été flushé dans l'index :
            // on l'ignore silencieusement s'il n'existe pas.
            if !seg_path.exists() {
                continue;
            }

            let reader = SegmentReader::open(&seg_path)?;

            // Étape 3 — filtre en mémoire : symbol + fenêtre temporelle
            for event in reader {
                if event.symbol == symbol
                    && event.timestamp_ns >= t_start
                    && event.timestamp_ns <= t_end
                {
                    results.push(event);
                }
            }
        }

        Ok(results)
    }

    /// Accès en lecture à l'index, utile pour l'inspection ou les tests.
    pub fn index(&self) -> &SegmentIndex {
        &self.index
    }

    // ── helpers privés ──────────────────────────────────────────────────────

    fn segment_path(&self, id: u32) -> PathBuf {
        self.dir.join(format!("{}{:04}{}", SEGMENT_PREFIX, id, SEGMENT_EXT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashta_core::{EventKind, SymbolId};
    use ashta_log::LogWriter;

    fn tmp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ashta_query_test_{}", name));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn make_event(ts: u64, symbol: &str) -> Event {
        Event {
            timestamp_ns: ts,
            symbol: SymbolId::from(symbol),
            price: 42.0,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        }
    }

    // Écrit des events via LogWriter, rotate (pour flush index), puis lit via QueryEngine.
    #[test]
    fn read_range_basic() {
        let dir = tmp_dir("basic");
        let btc = SymbolId::from("BTC/USD");
        let eth = SymbolId::from("ETH/USD");

        {
            let mut writer = LogWriter::open(&dir).unwrap();
            writer.append(&make_event(1_000, "BTC/USD")).unwrap();
            writer.append(&make_event(2_000, "ETH/USD")).unwrap();
            writer.append(&make_event(3_000, "BTC/USD")).unwrap();
            writer.append(&make_event(4_000, "BTC/USD")).unwrap();
            writer.rotate().unwrap(); // seal + flush index
        }

        let engine = QueryEngine::open(&dir).unwrap();

        // BTC dans [1000, 3000] → ts=1000 et ts=3000 (pas ts=4000)
        let events = engine.read_range(btc, 1_000, 3_000).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].timestamp_ns, 1_000);
        assert_eq!(events[1].timestamp_ns, 3_000);

        // ETH dans [0, 10_000] → ts=2000
        let eth_events = engine.read_range(eth, 0, 10_000).unwrap();
        assert_eq!(eth_events.len(), 1);
        assert_eq!(eth_events[0].timestamp_ns, 2_000);
    }

    // Le zone map doit éliminer les segments hors plage — on vérifie ça
    // en mettant deux segments avec des plages temporelles disjointes.
    #[test]
    fn zone_map_pruning_eliminates_out_of_range_segments() {
        let dir = tmp_dir("pruning");
        let btc = SymbolId::from("BTC/USD");

        {
            let mut writer = LogWriter::open(&dir).unwrap();

            // Segment 0 : ts 1000–3000
            writer.append(&make_event(1_000, "BTC/USD")).unwrap();
            writer.append(&make_event(2_000, "BTC/USD")).unwrap();
            writer.append(&make_event(3_000, "BTC/USD")).unwrap();
            writer.rotate().unwrap(); // → scelle seg 0, flush index

            // Segment 1 : ts 10_000–12_000
            writer.append(&make_event(10_000, "BTC/USD")).unwrap();
            writer.append(&make_event(11_000, "BTC/USD")).unwrap();
            writer.append(&make_event(12_000, "BTC/USD")).unwrap();
            writer.rotate().unwrap(); // → scelle seg 1, flush index
        }

        let engine = QueryEngine::open(&dir).unwrap();

        // Requête dans [10_000, 12_000] → le segment 0 (max_ts=3000) est éliminé par le zone map
        let candidates = engine.index().query(btc, 10_000, 12_000);
        assert_eq!(candidates, vec![1], "segment 0 doit être éliminé par le zone map");

        let events = engine.read_range(btc, 10_000, 12_000).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].timestamp_ns, 10_000);
    }

    // Sans aucun event correspondant, on doit obtenir un Vec vide (pas d'erreur).
    #[test]
    fn read_range_empty_result() {
        let dir = tmp_dir("empty_result");

        {
            let mut writer = LogWriter::open(&dir).unwrap();
            writer.append(&make_event(1_000, "ETH/USD")).unwrap();
            writer.rotate().unwrap();
        }

        let engine = QueryEngine::open(&dir).unwrap();
        let btc = SymbolId::from("BTC/USD");

        // Aucun event BTC dans ce log
        let events = engine.read_range(btc, 0, 100_000).unwrap();
        assert!(events.is_empty());
    }

    // Requête multi-segments : les résultats doivent être triés par segment (ordre d'écriture).
    #[test]
    fn read_range_spans_multiple_segments() {
        let dir = tmp_dir("multi_seg");
        let btc = SymbolId::from("BTC/USD");

        {
            let mut writer = LogWriter::open(&dir).unwrap();

            writer.append(&make_event(100, "BTC/USD")).unwrap();
            writer.append(&make_event(200, "BTC/USD")).unwrap();
            writer.rotate().unwrap();

            writer.append(&make_event(300, "BTC/USD")).unwrap();
            writer.append(&make_event(400, "BTC/USD")).unwrap();
            writer.rotate().unwrap();

            writer.append(&make_event(500, "BTC/USD")).unwrap();
            writer.append(&make_event(600, "BTC/USD")).unwrap();
            writer.rotate().unwrap();
        }

        let engine = QueryEngine::open(&dir).unwrap();

        // Plage [150, 450] → doit toucher seg 0 (max=200), seg 1 (100–400), seg 2 (min=500 exclu)
        let events = engine.read_range(btc, 150, 450).unwrap();
        let ts: Vec<u64> = events.iter().map(|e| e.timestamp_ns).collect();
        assert_eq!(ts, vec![200, 300, 400]);
    }
}
