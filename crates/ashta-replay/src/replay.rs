use std::io;
use std::path::Path;

use ashta_core::{Event, SymbolId};
use ashta_index::SegmentIndex;
use ashta_log::SegmentReader;

const SEGMENT_PREFIX: &str = "segment_";
const SEGMENT_EXT: &str = ".alog";
const INDEX_FILE: &str = "index.bin";

// ─── Configuration ────────────────────────────────────────────────────────────

/// Paramètres d'un replay.
///
/// Construire via les méthodes builder sur [`ReplayConfig`].
///
/// Invariants :
/// - `t_start <= t_end` (validé à l'ouverture de [`ReplayIter`])
/// - Si `symbols` est vide, tous les symbols sont rejoués
pub struct ReplayConfig {
    /// Borne inférieure (fermée) en nanosecondes. `0` = début absolu.
    pub t_start: u64,
    /// Borne supérieure (fermée) en nanosecondes. `u64::MAX` = fin absolue.
    pub t_end: u64,
    /// Filtre sur les symbols. Vide = tous les symbols.
    pub symbols: Vec<SymbolId>,
}

impl Default for ReplayConfig {
    /// Replay sans contrainte : tout l'historique, tous les symbols.
    fn default() -> Self {
        Self {
            t_start: 0,
            t_end: u64::MAX,
            symbols: Vec::new(),
        }
    }
}

impl ReplayConfig {
    /// Nouveau config avec la fenêtre temporelle `[t_start, t_end]`.
    pub fn with_range(mut self, t_start: u64, t_end: u64) -> Self {
        self.t_start = t_start;
        self.t_end = t_end;
        self
    }

    /// Ajoute un symbol au filtre.
    ///
    /// Peut être appelé plusieurs fois pour un replay multi-symbol.
    pub fn with_symbol(mut self, symbol: SymbolId) -> Self {
        self.symbols.push(symbol);
        self
    }

    /// Ouvre un [`ReplayIter`] sur le répertoire de log `dir`.
    pub fn open(self, dir: impl AsRef<Path>) -> io::Result<ReplayIter> {
        ReplayIter::new(dir, self)
    }
}

// ─── Itérateur ────────────────────────────────────────────────────────────────

/// Itère sur les events d'un log Ashta-TS en ordre temporel croissant.
///
/// Garanties :
/// - Les events sont émis dans l'ordre `timestamp_ns` croissant *par segment*.
///   Les segments sont parcourus dans l'ordre croissant de leur id → ordre
///   global garanti si `LogWriter` a maintenu l'invariant de monotonie.
/// - Seuls les segments candidats (via zone map) sont ouverts — pruning O(1).
/// - Aucun état résiduel entre deux `next()` — lecture pure, sans seek.
///
/// # Utilisation
///
/// ```rust,no_run
/// use ashta_replay::ReplayConfig;
/// use ashta_core::SymbolId;
///
/// let sym = SymbolId::from("BTCUSDT");
/// let iter = ReplayConfig::default()
///     .with_symbol(sym)
///     .with_range(1_000_000_000, 5_000_000_000)
///     .open("data/log")
///     .unwrap();
///
/// for event in iter {
///     println!("{:?}", event);
/// }
/// ```
pub struct ReplayIter {
    /// Segments candidats, dans l'ordre croissant — déjà chargés en mémoire.
    /// On draine `segments` de gauche à droite.
    segments: std::collections::VecDeque<SegmentReader>,
    /// L'itérateur actif sur le segment courant.
    current: Option<SegmentReader>,
    config: ReplayConfig,
}

impl ReplayIter {
    fn new(dir: impl AsRef<Path>, config: ReplayConfig) -> io::Result<Self> {
        let dir = dir.as_ref();

        // Valide l'invariant t_start <= t_end
        if config.t_start > config.t_end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "t_start ({}) > t_end ({}) — fenêtre temporelle invalide",
                    config.t_start, config.t_end
                ),
            ));
        }

        // Charge l'index — s'il n'existe pas, on liste tous les segments
        let index_path = dir.join(INDEX_FILE);
        let candidate_ids = if index_path.exists() {
            let index = SegmentIndex::open(&index_path)?;
            Self::candidates_from_index(&index, &config)
        } else {
            Self::candidates_from_dir(dir)?
        };

        // Ouvre les segments candidats dans l'ordre
        let mut segments = std::collections::VecDeque::new();
        for seg_id in candidate_ids {
            let path = dir.join(format!("{}{:04}{}", SEGMENT_PREFIX, seg_id, SEGMENT_EXT));
            if path.exists() {
                segments.push_back(SegmentReader::open(&path)?);
            }
        }

        // Initialise l'itérateur sur le premier segment
        let current = segments.pop_front();

        Ok(Self {
            segments,
            current,
            config,
        })
    }

    /// Utilise l'index (zone map) pour déterminer les segments candidats.
    ///
    /// Si le filtre symbols est vide, on requête tous les symbols distincts
    /// présents dans l'index. Si l'index est vide, tous les segments connus
    /// sont retournés (fallback sans pruning).
    fn candidates_from_index(index: &SegmentIndex, config: &ReplayConfig) -> Vec<u32> {
        if index.is_empty() {
            return Vec::new();
        }

        let mut all_ids = std::collections::BTreeSet::new();

        if config.symbols.is_empty() {
            // Pas de filtre symbol — on récupère tous les segments qui intersectent [t_start, t_end]
            for seg_id in index.query_all(config.t_start, config.t_end) {
                all_ids.insert(seg_id);
            }
        } else {
            for &sym in &config.symbols {
                for seg_id in index.query(sym, config.t_start, config.t_end) {
                    all_ids.insert(seg_id);
                }
            }
        }

        all_ids.into_iter().collect()
    }

    /// Fallback : liste les fichiers `.alog` du répertoire et extrait les ids.
    fn candidates_from_dir(dir: &Path) -> io::Result<Vec<u32>> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(SEGMENT_PREFIX) && name.ends_with(SEGMENT_EXT) {
                // "segment_00000000.alog" → extrait "00000000"
                let id_str = &name[SEGMENT_PREFIX.len()..name.len() - SEGMENT_EXT.len()];
                if let Ok(id) = id_str.parse::<u32>() {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Teste si un event passe les filtres de la config.
    #[inline]
    fn matches(&self, event: &Event) -> bool {
        // Filtre temporel
        if event.timestamp_ns < self.config.t_start || event.timestamp_ns > self.config.t_end {
            return false;
        }
        // Filtre symbol (vide = tous)
        if !self.config.symbols.is_empty() && !self.config.symbols.contains(&event.symbol) {
            return false;
        }
        true
    }
}

impl Iterator for ReplayIter {
    type Item = Event;

    /// Retourne le prochain event correspondant aux filtres, ou `None` si épuisé.
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Si plus de segment courant, on a terminé
            let current = self.current.as_mut()?;

            match current.next() {
                Some(event) if self.matches(&event) => return Some(event),
                Some(_) => {
                    // Event filtré — on continue dans le même segment
                    continue;
                }
                None => {
                    // Segment épuisé — passe au suivant
                    self.current = self.segments.pop_front();
                }
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ashta_core::{EventKind, SymbolId};
    use ashta_log::LogWriter;
    use std::path::PathBuf;
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ashta_replay_test_{}", name));
        let _ = fs::remove_dir_all(&p);
        p
    }

    fn make_event(ts_ns: u64, symbol: &str, price: f64) -> Event {
        Event {
            timestamp_ns: ts_ns,
            symbol: SymbolId::from(symbol),
            price,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        }
    }

    /// Écrit des events dans un log et scelle le dernier segment.
    fn write_log(dir: &Path, events: &[Event]) -> io::Result<()> {
        let mut writer = LogWriter::open(dir)?;
        for e in events {
            writer.append(e)?;
        }
        writer.rotate()?;
        Ok(())
    }

    // ── replay complet (sans filtre) ──────────────────────────────────────

    #[test]
    fn replay_all_events_in_order() {
        let dir = tmp_dir("all");
        let events = vec![
            make_event(1_000, "BTCUSDT", 43000.0),
            make_event(2_000, "ETHUSDT", 2300.0),
            make_event(3_000, "BTCUSDT", 43100.0),
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default().open(&dir).unwrap().collect();

        assert_eq!(replayed.len(), 3);
        // Ordre temporel respecté
        assert!(replayed[0].timestamp_ns <= replayed[1].timestamp_ns);
        assert!(replayed[1].timestamp_ns <= replayed[2].timestamp_ns);
    }

    // ── filtre par symbol ─────────────────────────────────────────────────

    #[test]
    fn replay_filters_by_symbol() {
        let dir = tmp_dir("symbol");
        let btc = SymbolId::from("BTCUSDT");
        let eth = SymbolId::from("ETHUSDT");
        let events = vec![
            make_event(1_000, "BTCUSDT", 43000.0),
            make_event(2_000, "ETHUSDT", 2300.0),
            make_event(3_000, "BTCUSDT", 43100.0),
            make_event(4_000, "ETHUSDT", 2310.0),
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default()
            .with_symbol(btc)
            .open(&dir)
            .unwrap()
            .collect();

        assert_eq!(replayed.len(), 2);
        assert!(replayed.iter().all(|e| e.symbol == btc));
        assert!(replayed.iter().all(|e| e.symbol != eth));
    }

    // ── filtre par fenêtre temporelle ─────────────────────────────────────

    #[test]
    fn replay_filters_by_time_range() {
        let dir = tmp_dir("range");
        let events = vec![
            make_event(1_000, "BTCUSDT", 43000.0), // avant
            make_event(2_000, "BTCUSDT", 43100.0), // dans
            make_event(3_000, "BTCUSDT", 43200.0), // dans
            make_event(4_000, "BTCUSDT", 43300.0), // après
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default()
            .with_range(2_000, 3_000)
            .open(&dir)
            .unwrap()
            .collect();

        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].timestamp_ns, 2_000);
        assert_eq!(replayed[1].timestamp_ns, 3_000);
    }

    // ── filtre combiné : symbol + range ───────────────────────────────────

    #[test]
    fn replay_combined_filter() {
        let dir = tmp_dir("combined");
        let btc = SymbolId::from("BTCUSDT");
        let events = vec![
            make_event(1_000, "BTCUSDT", 43000.0),
            make_event(2_000, "ETHUSDT", 2300.0),
            make_event(3_000, "BTCUSDT", 43200.0),
            make_event(4_000, "BTCUSDT", 43300.0),
            make_event(5_000, "ETHUSDT", 2400.0),
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default()
            .with_symbol(btc)
            .with_range(2_000, 4_000)
            .open(&dir)
            .unwrap()
            .collect();

        assert_eq!(replayed.len(), 2);
        assert!(replayed.iter().all(|e| e.symbol == btc));
        assert_eq!(replayed[0].timestamp_ns, 3_000);
        assert_eq!(replayed[1].timestamp_ns, 4_000);
    }

    // ── fenêtre vide ──────────────────────────────────────────────────────

    #[test]
    fn replay_empty_range_returns_nothing() {
        let dir = tmp_dir("empty_range");
        let events = vec![
            make_event(5_000, "BTCUSDT", 43000.0),
            make_event(6_000, "BTCUSDT", 43100.0),
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default()
            .with_range(1_000, 2_000) // avant tous les events
            .open(&dir)
            .unwrap()
            .collect();

        assert_eq!(replayed.len(), 0);
    }

    // ── t_start > t_end → erreur ──────────────────────────────────────────

    #[test]
    fn replay_invalid_range_returns_error() {
        let dir = tmp_dir("invalid_range");
        fs::create_dir_all(&dir).unwrap();

        let result = ReplayConfig::default()
            .with_range(9_000, 1_000) // invalide
            .open(&dir);

        assert!(result.is_err());
        match result {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidInput),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    // ── replay multi-symbol ───────────────────────────────────────────────

    #[test]
    fn replay_multi_symbol_filter() {
        let dir = tmp_dir("multi_sym");
        let btc = SymbolId::from("BTCUSDT");
        let eth = SymbolId::from("ETHUSDT");
        let events = vec![
            make_event(1_000, "BTCUSDT", 43000.0),
            make_event(2_000, "ETHUSDT", 2300.0),
            make_event(3_000, "SOLUSDT", 150.0), // filtré
            make_event(4_000, "BTCUSDT", 43100.0),
        ];
        write_log(&dir, &events).unwrap();

        let replayed: Vec<Event> = ReplayConfig::default()
            .with_symbol(btc)
            .with_symbol(eth)
            .open(&dir)
            .unwrap()
            .collect();

        assert_eq!(replayed.len(), 3);
        assert!(replayed.iter().all(|e| e.symbol == btc || e.symbol == eth));
    }
}
