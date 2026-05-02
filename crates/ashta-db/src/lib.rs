use std::io;
use std::path::{Path, PathBuf};

use ashta_core::{Event, SymbolId};
use ashta_log::LogWriter;
use ashta_query::QueryEngine;

/// Point d'entrée unique du storage engine Ashta-TS.
///
/// `AshtaDB` orchestre la couche d'écriture (`LogWriter`) et la couche de
/// lecture (`QueryEngine`) sur un même répertoire de données. L'appelant n'a
/// pas besoin de connaître les crates internes — l'API se résume à quatre
/// opérations : `open`, `write`, `read_range`, `flush`.
///
/// # Modèle de cohérence
///
/// Les events écrits via `write()` sont visibles pour `read_range()` **après**
/// un appel à `flush()`, qui scelle le segment actif et persiste l'index.
/// Sans flush, le segment actif est en cours d'écriture et non indexé — les
/// queries ne retourneront pas les events les plus récents.
///
/// # Exemple
///
/// ```no_run
/// use ashta_db::AshtaDB;
/// use ashta_core::{Event, EventKind, SymbolId};
///
/// let mut db = AshtaDB::open("/var/ashta/btc").unwrap();
///
/// let event = Event {
///     timestamp_ns: 1_700_000_000_000_000_000,
///     symbol: SymbolId::from("BTCUSDT"),
///     price: 67_432.10,
///     volume: 0.001,
///     kind: EventKind::Trade,
///     _pad: [0; 7],
/// };
///
/// db.write(&event).unwrap();
/// db.flush().unwrap();
///
/// let results = db.read_range(
///     SymbolId::from("BTCUSDT"),
///     1_700_000_000_000_000_000,
///     1_700_000_001_000_000_000,
/// ).unwrap();
/// ```
pub struct AshtaDB {
    dir: PathBuf,
    writer: LogWriter,
}

impl AshtaDB {
    /// Ouvre (ou crée) la base de données dans `dir`.
    ///
    /// Reprend sur le dernier segment et recharge l'index depuis `index.bin`.
    /// Crée le répertoire s'il n'existe pas.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si le répertoire ne peut pas être créé, ou si le
    /// dernier segment est corrompu.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let writer = LogWriter::open(&dir)?;
        Ok(Self { dir, writer })
    }

    /// Écrit un event dans le segment actif.
    ///
    /// Déclenche automatiquement une rotation si le segment atteint 64 MiB
    /// (≈ 1.6 M events). La rotation scelle le segment et persiste l'index —
    /// les events sont alors immédiatement queryables.
    ///
    /// # Errors
    ///
    /// Retourne une erreur en cas d'échec I/O (disque plein, permissions, etc.)
    pub fn write(&mut self, event: &Event) -> io::Result<()> {
        self.writer.append(event)
    }

    /// Retourne tous les events de `symbol` dont le timestamp est dans
    /// l'intervalle **fermé** `[t_start, t_end]` (en nanosecondes).
    ///
    /// Seuls les segments déjà **scellés** (visibles dans l'index) sont
    /// interrogés. Les events du segment actif non encore flushé ne sont
    /// pas inclus — appeler `flush()` d'abord si nécessaire.
    ///
    /// Utilise le mmap read path : zéro copie depuis le page cache kernel.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si un segment candidat ne peut pas être ouvert.
    pub fn read_range(
        &self,
        symbol: SymbolId,
        t_start: u64,
        t_end: u64,
    ) -> io::Result<Vec<Event>> {
        let engine = QueryEngine::open(&self.dir)?;
        engine.read_range(symbol, t_start, t_end)
    }

    /// Scelle le segment actif et persiste l'index sur disque.
    ///
    /// Après cet appel, tous les events écrits sont durables et queryables.
    /// Un nouveau segment vide est ouvert pour les prochains appels à `write()`.
    ///
    /// À appeler périodiquement (toutes les N secondes, ou à l'arrêt propre).
    ///
    /// # Errors
    ///
    /// Retourne une erreur si le flush disque (fsync) échoue.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.rotate()
    }

    /// Nombre d'events dans le segment actif (non encore flushés).
    pub fn pending_events(&self) -> u64 {
        self.writer.current_event_count()
    }

    /// Identifiant du segment actif.
    pub fn current_segment_id(&self) -> u32 {
        self.writer.current_segment_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashta_core::{EventKind, SymbolId};

    fn tmp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ashta_db_test_{}", name));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn make_event(ts: u64, symbol: &str, price: f64) -> Event {
        Event {
            timestamp_ns: ts,
            symbol: SymbolId::from(symbol),
            price,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        }
    }

    #[test]
    fn open_creates_dir() {
        let dir = tmp_dir("open");
        assert!(AshtaDB::open(&dir).is_ok());
        assert!(dir.exists());
    }

    #[test]
    fn write_increments_pending() {
        let dir = tmp_dir("pending");
        let mut db = AshtaDB::open(&dir).unwrap();

        assert_eq!(db.pending_events(), 0);
        db.write(&make_event(1_000, "BTC", 60_000.0)).unwrap();
        db.write(&make_event(2_000, "BTC", 60_001.0)).unwrap();
        assert_eq!(db.pending_events(), 2);
    }

    #[test]
    fn flush_seals_and_resets_pending() {
        let dir = tmp_dir("flush");
        let mut db = AshtaDB::open(&dir).unwrap();

        db.write(&make_event(1_000, "BTC", 60_000.0)).unwrap();
        db.flush().unwrap();

        // Après flush : segment_id monte, pending revient à 0
        assert_eq!(db.pending_events(), 0);
        assert_eq!(db.current_segment_id(), 1);
    }

    #[test]
    fn write_flush_read_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let mut db = AshtaDB::open(&dir).unwrap();
        let btc = SymbolId::from("BTCUSDT");

        db.write(&make_event(1_000_000, "BTCUSDT", 67_000.0)).unwrap();
        db.write(&make_event(2_000_000, "BTCUSDT", 67_001.0)).unwrap();
        db.write(&make_event(3_000_000, "ETHUSDT", 3_000.0)).unwrap();
        db.flush().unwrap();

        let results = db.read_range(btc, 0, 5_000_000).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].timestamp_ns, 1_000_000);
        assert_eq!(results[1].timestamp_ns, 2_000_000);
        assert!((results[0].price - 67_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn read_range_empty_when_no_flush() {
        let dir = tmp_dir("no_flush");
        let mut db = AshtaDB::open(&dir).unwrap();
        let btc = SymbolId::from("BTCUSDT");

        // Events écrits mais pas flushés → pas encore dans l'index
        db.write(&make_event(1_000_000, "BTCUSDT", 67_000.0)).unwrap();

        let results = db.read_range(btc, 0, u64::MAX).unwrap();
        assert!(results.is_empty(), "segment actif non scellé ne doit pas être queryable");
    }

    #[test]
    fn reopen_preserves_data() {
        let dir = tmp_dir("reopen");
        let btc = SymbolId::from("BTCUSDT");

        {
            let mut db = AshtaDB::open(&dir).unwrap();
            db.write(&make_event(1_000_000, "BTCUSDT", 67_000.0)).unwrap();
            db.flush().unwrap();
        }

        // Réouverture : les données doivent survivre
        let db = AshtaDB::open(&dir).unwrap();
        let results = db.read_range(btc, 0, u64::MAX).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].timestamp_ns, 1_000_000);
    }
}
