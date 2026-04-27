use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ashta_core::event::Event;
use ashta_index::SegmentIndex;

use crate::segment::{SealedSegment, SegmentWriter};

const SEGMENT_PREFIX: &str = "segment_";
const SEGMENT_EXT: &str = ".alog";
const INDEX_FILE: &str = "index.bin";

/// Orchestre l'écriture append-only sur une séquence de segments,
/// couplée à la mise à jour du `SegmentIndex`.
///
/// Invariants :
/// - Les segments sont numérotés sans trou (0, 1, 2, ...)
/// - Un seul segment est actif à la fois (le plus grand numéro)
/// - L'index est flushé sur disque à chaque seal de segment
/// - Au redémarrage, on reprend sur le dernier segment ET recharge l'index
pub struct LogWriter {
    dir: PathBuf,
    current: SegmentWriter,
    segment_id: u32,
    index: SegmentIndex,
    sealed: Vec<SealedSegment>,
}

impl LogWriter {
    /// Ouvre ou crée un log dans `dir`.
    ///
    /// Reprend sur le dernier segment et recharge l'index depuis `index.bin`.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;

        let segment_id = Self::find_last_segment_id(&dir)?;
        let path = Self::segment_path(&dir, segment_id);
        let current = SegmentWriter::open(&path)?;

        let index_path = dir.join(INDEX_FILE);
        let index = SegmentIndex::open(&index_path)?;

        Ok(Self {
            dir,
            current,
            segment_id,
            index,
            sealed: Vec::new(),
        })
    }

    /// Écrit un `Event` dans le segment actif et met à jour l'index en mémoire.
    /// Si le segment est plein, le scelle (+ flush index) et ouvre le suivant.
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        if self.current.is_full() {
            self.rotate()?;
        }
        self.index.observe(event.symbol, self.segment_id, event.timestamp_ns);
        self.current.write_event(event)
    }

    /// Accès en lecture à l'index — pour les requêtes dans `ashta-query`.
    pub fn index(&self) -> &SegmentIndex {
        &self.index
    }

    pub fn current_event_count(&self) -> u64 {
        self.current.event_count()
    }

    pub fn current_segment_id(&self) -> u32 {
        self.segment_id
    }

    pub fn sealed_segments(&self) -> &[SealedSegment] {
        &self.sealed
    }

    /// Scelle le segment actif, flush l'index, et ouvre le suivant.
    pub fn rotate(&mut self) -> io::Result<()> {
        let next_id = self.segment_id + 1;
        let next_path = Self::segment_path(&self.dir, next_id);
        let next_writer = SegmentWriter::open(&next_path)?;

        let old_writer = std::mem::replace(&mut self.current, next_writer);
        self.segment_id = next_id;

        // 1. Scelle les données (flush + fsync)
        let sealed = old_writer.seal()?;
        self.sealed.push(sealed);

        // 2. Flush l'index sur disque (fsync)
        self.index.flush()?;

        Ok(())
    }

    // ── helpers privés ──────────────────────────────────────────────────────

    fn segment_path(dir: &Path, id: u32) -> PathBuf {
        dir.join(format!("{}{:04}{}", SEGMENT_PREFIX, id, SEGMENT_EXT))
    }

    fn find_last_segment_id(dir: &Path) -> io::Result<u32> {
        let mut max_id: u32 = 0;
        let mut found = false;

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = Self::parse_segment_name(&name) {
                if !found || id > max_id {
                    max_id = id;
                    found = true;
                }
            }
        }

        Ok(max_id)
    }

    fn parse_segment_name(name: &str) -> Option<u32> {
        let name = name.strip_prefix(SEGMENT_PREFIX)?;
        let name = name.strip_suffix(SEGMENT_EXT)?;
        name.parse::<u32>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashta_core::{Event, EventKind, SymbolId};

    fn tmp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ashta_log_test_{}", name));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn make_event(ts: u64, symbol: &str) -> Event {
        Event {
            timestamp_ns: ts,
            symbol: SymbolId::from(symbol),
            price: 100.0,
            volume: 1.0,
            kind: EventKind::Trade,
            _pad: [0; 7],
        }
    }

    #[test]
    fn open_empty_dir_creates_segment_0000() {
        let dir = tmp_dir("empty");
        let writer = LogWriter::open(&dir).unwrap();
        assert_eq!(writer.current_segment_id(), 0);
        assert_eq!(writer.current_event_count(), 0);
        assert!(dir.join("segment_0000.alog").exists());
    }

    #[test]
    fn append_events_and_read_back() {
        let dir = tmp_dir("append");
        let mut writer = LogWriter::open(&dir).unwrap();

        writer.append(&make_event(1_000, "BTC/USD")).unwrap();
        writer.append(&make_event(2_000, "ETH/USD")).unwrap();
        writer.append(&make_event(3_000, "BTC/USD")).unwrap();

        assert_eq!(writer.current_event_count(), 3);
        assert_eq!(writer.current_segment_id(), 0);

        writer.rotate().unwrap();
        assert_eq!(writer.current_segment_id(), 1);
        assert_eq!(writer.sealed_segments().len(), 1);

        let sealed_path = &writer.sealed_segments()[0].path;
        let reader = crate::segment::SegmentReader::open(sealed_path).unwrap();
        let events: Vec<Event> = reader.collect();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].timestamp_ns, 1_000);
        assert_eq!(events[1].symbol, SymbolId::from("ETH/USD"));
        assert_eq!(events[2].timestamp_ns, 3_000);
    }

    #[test]
    fn reopen_resumes_on_last_segment() {
        let dir = tmp_dir("resume");

        {
            let mut writer = LogWriter::open(&dir).unwrap();
            writer.append(&make_event(1_000, "AAPL")).unwrap();
            writer.append(&make_event(2_000, "AAPL")).unwrap();
            writer.rotate().unwrap();
            writer.append(&make_event(3_000, "AAPL")).unwrap();
        }

        let writer2 = LogWriter::open(&dir).unwrap();
        assert_eq!(writer2.current_segment_id(), 1);
        assert_eq!(writer2.current_event_count(), 1);
    }

    #[test]
    fn index_updated_on_append_and_survives_reload() {
        let dir = tmp_dir("index_coupled");
        let btc = SymbolId::from("BTC/USD");
        let eth = SymbolId::from("ETH/USD");

        // Session 1 : écrit des events et rotate (force flush index)
        {
            let mut writer = LogWriter::open(&dir).unwrap();
            writer.append(&make_event(1_000, "BTC/USD")).unwrap();
            writer.append(&make_event(5_000, "ETH/USD")).unwrap();
            writer.append(&make_event(9_000, "BTC/USD")).unwrap();
            // rotate : seal segment_0000 + flush index
            writer.rotate().unwrap();

            // L'index en mémoire doit déjà connaître les deux symbols
            let idx = writer.index();
            let btc_segs = idx.query(btc, 0, 100_000);
            assert_eq!(btc_segs, vec![0]);
            let eth_segs = idx.query(eth, 0, 100_000);
            assert_eq!(eth_segs, vec![0]);
        }

        // Session 2 : l'index est rechargé depuis index.bin
        let writer2 = LogWriter::open(&dir).unwrap();
        let idx = writer2.index();

        // BTC dans [0, 100_000] → segment 0
        assert_eq!(idx.query(btc, 0, 100_000), vec![0]);
        // ETH dans [0, 100_000] → segment 0
        assert_eq!(idx.query(eth, 0, 100_000), vec![0]);
        // BTC dans [100_000, 200_000] → rien (pas encore d'events là)
        assert!(idx.query(btc, 100_000, 200_000).is_empty());
    }

    #[test]
    fn parse_segment_name_works() {
        assert_eq!(LogWriter::parse_segment_name("segment_0000.alog"), Some(0));
        assert_eq!(LogWriter::parse_segment_name("segment_0042.alog"), Some(42));
        assert_eq!(LogWriter::parse_segment_name("not_a_segment.txt"), None);
        assert_eq!(LogWriter::parse_segment_name("segment_abc.alog"), None);
    }
}
