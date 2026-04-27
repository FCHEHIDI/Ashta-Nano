use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ashta_core::event::Event;

/// Taille maximale d'un segment avant rotation : 64 MiB.
/// Soit 64 * 1024 * 1024 / 40 = 1_677_721 events par segment.
pub const SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Écrit des `Event` dans un segment append-only sur disque.
///
/// Invariants :
/// - Écriture séquentielle uniquement — pas de seek, pas de mise à jour in-place
/// - Chaque `write_event` est atomique au niveau du record (40 bytes)
/// - `seal()` garantit que toutes les données sont sur disque (fsync)
/// - Un `SegmentWriter` scellé ne peut plus être utilisé
pub struct SegmentWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    bytes_written: u64,
    sealed: bool,
}

impl SegmentWriter {
    /// Ouvre ou crée un segment à `path` en mode append.
    ///
    /// Si le fichier existe déjà, on reprend après le dernier byte écrit.
    /// Utile pour la recovery après crash.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        // Taille actuelle = bytes déjà écrits (recovery après crash)
        let bytes_written = file.metadata()?.len();

        Ok(Self {
            path,
            writer: BufWriter::new(file),
            bytes_written,
            sealed: false,
        })
    }

    /// Écrit un `Event` dans le segment.
    ///
    /// # Errors
    /// Retourne une erreur si le segment est déjà scellé ou si l'I/O échoue.
    ///
    /// # Safety (interne)
    /// On interprète les bytes de `Event` directement.
    /// C'est safe car `Event` est `repr(C)` avec taille fixe connue à la compilation.
    pub fn write_event(&mut self, event: &Event) -> io::Result<()> {
        if self.sealed {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "cannot write to a sealed segment",
            ));
        }

        // SAFETY: Event est repr(C), taille = 40 bytes, aligné 8.
        // Pas de pointeurs internes, pas de padding non-initialisé.
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                (event as *const Event) as *const u8,
                std::mem::size_of::<Event>(),
            )
        };

        self.writer.write_all(bytes)?;
        self.bytes_written += std::mem::size_of::<Event>() as u64;
        Ok(())
    }

    /// Nombre de bytes écrits dans ce segment.
    #[inline]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Nombre d'events écrits dans ce segment.
    #[inline]
    pub fn event_count(&self) -> u64 {
        self.bytes_written / std::mem::size_of::<Event>() as u64
    }

    /// Indique si le segment a atteint sa taille maximale.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.bytes_written >= SEGMENT_MAX_BYTES
    }

    /// Scelle le segment : flush le buffer, puis fsync sur disque.
    ///
    /// Après `seal()`, le segment est immuable — plus aucune écriture n'est possible.
    /// Cette opération garantit la durabilité : un crash après `seal()` ne perd rien.
    pub fn seal(mut self) -> io::Result<SealedSegment> {
        // 1. Flush le BufWriter vers le file descriptor
        self.writer.flush()?;

        // 2. Récupérer le File depuis le BufWriter pour fsync
        let file = self.writer.into_inner().map_err(|e| e.into_error())?;

        // 3. fsync : force l'écriture du page cache vers le disque physique
        file.sync_all()?;

        Ok(SealedSegment {
            path: self.path,
            event_count: self.bytes_written / std::mem::size_of::<Event>() as u64,
        })
    }
}

/// Représente un segment scellé (immuable, garanti sur disque).
#[derive(Debug, Clone)]
pub struct SealedSegment {
    pub path: PathBuf,
    pub event_count: u64,
}

/// Lit des `Event` depuis un segment existant sur disque.
///
/// Lecture séquentielle par itération. Le segment doit avoir été produit
/// par `SegmentWriter` — aucune validation de format (layout fixe attendu).
pub struct SegmentReader {
    data: Vec<u8>,
    cursor: usize,
}

impl SegmentReader {
    /// Charge le segment en mémoire depuis `path`.
    ///
    /// Pour un MVP on charge tout en RAM. On passera à `mmap` dans `ashta-mem`.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(Self { data, cursor: 0 })
    }

    /// Nombre total d'events dans ce segment.
    pub fn event_count(&self) -> usize {
        self.data.len() / std::mem::size_of::<Event>()
    }
}

impl Iterator for SegmentReader {
    type Item = Event;

    /// Retourne le prochain `Event`, ou `None` si fin de segment.
    ///
    /// # Safety (interne)
    /// On reinterprète 40 bytes en `Event`.
    /// Safe car le fichier a été écrit par `SegmentWriter` avec le même layout.
    fn next(&mut self) -> Option<Self::Item> {
        let size = std::mem::size_of::<Event>();
        if self.cursor + size > self.data.len() {
            return None;
        }

        let bytes = &self.data[self.cursor..self.cursor + size];
        self.cursor += size;

        // SAFETY: les bytes viennent d'un SegmentWriter — layout Event garanti.
        let event = unsafe {
            let ptr = bytes.as_ptr() as *const Event;
            *ptr
        };

        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashta_core::{Event, EventKind, SymbolId};

    fn make_event(ts: u64, symbol: &str, price: f64, volume: f64) -> Event {
        Event {
            timestamp_ns: ts,
            symbol: SymbolId::from(symbol),
            price,
            volume,
            kind: EventKind::Trade,
            _pad: [0; 7],
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let path = std::env::temp_dir().join("ashta_test_segment.alog");

        // Écriture
        let mut writer = SegmentWriter::open(&path).unwrap();
        writer.write_event(&make_event(1_000, "BTC/USD", 60_000.0, 1.5)).unwrap();
        writer.write_event(&make_event(2_000, "BTC/USD", 60_001.0, 0.3)).unwrap();
        writer.write_event(&make_event(3_000, "ETH/USD", 3_000.0, 10.0)).unwrap();

        assert_eq!(writer.event_count(), 3);
        assert_eq!(writer.bytes_written(), 120); // 3 * 40

        let _sealed = writer.seal().unwrap();

        // Lecture
        let reader = SegmentReader::open(&path).unwrap();
        assert_eq!(reader.event_count(), 3);

        let events: Vec<Event> = reader.collect();
        assert_eq!(events[0].timestamp_ns, 1_000);
        assert_eq!(events[0].symbol, SymbolId::from("BTC/USD"));
        assert_eq!(events[0].price, 60_000.0);

        assert_eq!(events[2].timestamp_ns, 3_000);
        assert_eq!(events[2].symbol, SymbolId::from("ETH/USD"));

        // Nettoyage
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sealed_segment_rejects_writes() {
        let path = std::env::temp_dir().join("ashta_test_sealed.alog");
        let mut writer = SegmentWriter::open(&path).unwrap();
        writer.write_event(&make_event(1_000, "AAPL", 200.0, 50.0)).unwrap();

        // On scelle — mais on ne peut pas réutiliser writer après seal()
        // car seal() consomme self. Le compilateur l'interdit statiquement.
        // Ce test vérifie juste que seal() réussit et retourne le bon compte.
        let sealed = writer.seal().unwrap();
        assert_eq!(sealed.event_count, 1);

        std::fs::remove_file(&path).ok();
    }
}
