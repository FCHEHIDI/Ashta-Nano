use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use ashta_core::symbol::SymbolId;

use crate::entry::IndexEntry;

/// Clé de l'index en mémoire : un symbol dans un segment précis.
type IndexKey = (SymbolId, u32); // (symbol, segment_id)

/// Index en mémoire des segments d'Ashta-TS.
///
/// Maintient une `IndexEntry` par `(symbol, segment_id)`.
/// Permet de répondre à la question :
/// "Quels segments dois-je ouvrir pour lire `symbol` entre `t_start` et `t_end` ?"
///
/// Persistance : sérialisé en binaire dans `index.bin` (tableau d'`IndexEntry`).
/// Reconstruction : si `index.bin` absent, on peut reconstruire en scannant les segments.
pub struct SegmentIndex {
    entries: HashMap<IndexKey, IndexEntry>,
    index_path: PathBuf,
}

impl SegmentIndex {
    /// Ouvre ou recrée un index à partir de `index_path`.
    ///
    /// Si le fichier existe, charge les entrées depuis le disque.
    /// Sinon, démarre avec un index vide.
    pub fn open(index_path: impl AsRef<Path>) -> io::Result<Self> {
        let index_path = index_path.as_ref().to_path_buf();
        let entries = if index_path.exists() {
            Self::load_entries(&index_path)?
        } else {
            HashMap::new()
        };

        Ok(Self {
            entries,
            index_path,
        })
    }

    /// Observe un event : met à jour ou crée l'entrée pour `(symbol, segment_id)`.
    ///
    /// À appeler par `LogWriter` à chaque `append()`.
    pub fn observe(&mut self, symbol: SymbolId, segment_id: u32, timestamp_ns: u64) {
        self.entries
            .entry((symbol, segment_id))
            .and_modify(|e| e.observe(timestamp_ns))
            .or_insert_with(|| IndexEntry::new(symbol, segment_id, timestamp_ns));
    }

    /// Retourne les ids de segments candidats pour une requête `(symbol, [t_start, t_end])`.
    ///
    /// Les ids sont retournés en ordre croissant pour une lecture séquentielle optimale.
    ///
    /// Un segment "candidat" peut contenir des events dans la plage — il faut le lire
    /// pour confirmer (l'index ne garantit pas l'absence de faux positifs, seulement
    /// l'absence de faux négatifs).
    pub fn query(&self, symbol: SymbolId, t_start: u64, t_end: u64) -> Vec<u32> {
        let mut candidates: Vec<u32> = self
            .entries
            .iter()
            .filter(|((sym, _), entry)| *sym == symbol && entry.overlaps(t_start, t_end))
            .map(|((_, seg_id), _)| *seg_id)
            .collect();

        candidates.sort_unstable();
        candidates
    }

    /// Retourne tous les ids de segments dont la zone map intersecte `[t_start, t_end]`,
    /// quel que soit le symbol.
    ///
    /// Utilisé par `ashta-replay` quand aucun filtre symbol n'est posé.
    pub fn query_all(&self, t_start: u64, t_end: u64) -> Vec<u32> {
        let mut candidates: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.overlaps(t_start, t_end))
            .map(|((_, seg_id), _)| *seg_id)
            .collect();

        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }

    /// Nombre total d'entrées dans l'index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Persiste l'index sur disque (tableau d'`IndexEntry` en repr(C)).
    ///
    /// Écrase le fichier existant. Doit être appelé au moins à chaque seal de segment.
    pub fn flush(&self) -> io::Result<()> {
        use std::io::Write;

        let mut file = std::fs::File::create(&self.index_path)?;

        for entry in self.entries.values() {
            // SAFETY: IndexEntry est repr(C), 40 bytes, pas de pointeurs internes.
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    (entry as *const IndexEntry) as *const u8,
                    std::mem::size_of::<IndexEntry>(),
                )
            };
            file.write_all(bytes)?;
        }

        file.sync_all()?;
        Ok(())
    }

    // ── helpers privés ──────────────────────────────────────────────────────

    fn load_entries(path: &Path) -> io::Result<HashMap<IndexKey, IndexEntry>> {
        let data = std::fs::read(path)?;
        let entry_size = std::mem::size_of::<IndexEntry>();

        if data.len() % entry_size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "index file size {} is not a multiple of IndexEntry size {}",
                    data.len(),
                    entry_size
                ),
            ));
        }

        let mut map = HashMap::new();
        for chunk in data.chunks_exact(entry_size) {
            // SAFETY: chunk est exactement entry_size bytes, provient d'un flush() précédent.
            let entry: IndexEntry = unsafe {
                let ptr = chunk.as_ptr() as *const IndexEntry;
                *ptr
            };
            map.insert((entry.symbol, entry.segment_id), entry);
        }

        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_index(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ashta_index_{}.bin", name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn observe_and_query_basic() {
        let path = tmp_index("basic");
        let mut idx = SegmentIndex::open(&path).unwrap();

        let btc = SymbolId::from("BTC/USD");
        let eth = SymbolId::from("ETH/USD");

        // segment 0 : BTC de 1_000 à 50_000, ETH de 2_000 à 40_000
        idx.observe(btc, 0, 1_000);
        idx.observe(btc, 0, 50_000);
        idx.observe(eth, 0, 2_000);
        idx.observe(eth, 0, 40_000);

        // segment 1 : BTC de 60_000 à 120_000
        idx.observe(btc, 1, 60_000);
        idx.observe(btc, 1, 120_000);

        // Requête BTC dans [10_000, 70_000] → doit toucher seg 0 ET seg 1
        let candidates = idx.query(btc, 10_000, 70_000);
        assert_eq!(candidates, vec![0, 1]); // triés

        // Requête BTC dans [130_000, 200_000] → aucun segment
        let candidates = idx.query(btc, 130_000, 200_000);
        assert!(candidates.is_empty());

        // Requête ETH dans [1_000, 5_000] → seg 0 seulement
        let candidates = idx.query(eth, 1_000, 5_000);
        assert_eq!(candidates, vec![0]);
    }

    #[test]
    fn flush_and_reload() {
        let path = tmp_index("persist");
        let btc = SymbolId::from("BTC/USD");

        // Première session : observe + flush
        {
            let mut idx = SegmentIndex::open(&path).unwrap();
            idx.observe(btc, 0, 1_000);
            idx.observe(btc, 0, 9_000);
            idx.observe(btc, 1, 10_000);
            idx.flush().unwrap();
        }

        // Deuxième session : recharge depuis disque
        let idx = SegmentIndex::open(&path).unwrap();
        assert_eq!(idx.len(), 2); // 2 entrées : (btc, 0) et (btc, 1)

        let candidates = idx.query(btc, 5_000, 15_000);
        assert_eq!(candidates, vec![0, 1]);

        // Les segments hors range ne sont pas retournés
        let candidates = idx.query(btc, 20_000, 30_000);
        assert!(candidates.is_empty());
    }

    #[test]
    fn query_returns_sorted_segment_ids() {
        let path = tmp_index("sorted");
        let mut idx = SegmentIndex::open(&path).unwrap();
        let sym = SymbolId::from("AAPL");

        // Insertion dans l'ordre inverse pour tester le tri
        idx.observe(sym, 4, 400);
        idx.observe(sym, 2, 200);
        idx.observe(sym, 0, 100);
        idx.observe(sym, 3, 300);
        idx.observe(sym, 1, 150);

        let candidates = idx.query(sym, 0, 1_000);
        assert_eq!(candidates, vec![0, 1, 2, 3, 4]);
    }
}
