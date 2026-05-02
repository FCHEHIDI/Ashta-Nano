use std::fs::File;
use std::io;
use std::path::Path;

use memmap2::Mmap;

/// Segment de log mappé en mémoire via `mmap(2)` (ou `MapViewOfFile` sur Windows).
///
/// Remplace le `fs::read()` → `Vec<u8>` de `SegmentReader` :
/// le kernel mappe directement les pages du fichier dans l'espace d'adressage —
/// zéro copie, zéro allocation heap, accès O(1) par offset.
///
/// # Invariant de sécurité
///
/// Un `MmapSegment` ne doit être ouvert que sur un segment **scellé** (immuable).
/// Les segments Ashta-TS sont scellés par `SegmentWriter::seal()` + `fsync`
/// avant d'être consommés en lecture. Aucune écriture concurrente n'est possible
/// sur un segment scellé → la contrainte SAFETY de `Mmap::map()` est respectée.
///
/// # Drop
///
/// Le `munmap` est effectué automatiquement au `Drop` de `Mmap` (memmap2).
/// Aucune ressource n'est laissée ouverte après destruction.
pub struct MmapSegment {
    mmap: Mmap,
}

impl MmapSegment {
    /// Ouvre et mappe le fichier `path` en lecture seule.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si le fichier n'existe pas, ne peut pas être ouvert,
    /// ou si l'appel système mmap échoue (ex. : espace d'adressage exhausted).
    ///
    /// # Safety note (interne)
    ///
    /// `Mmap::map()` est `unsafe` car le comportement est indéfini si le fichier
    /// sous-jacent est modifié pendant la durée de vie du mapping.
    /// Ici c'est safe : on n'ouvre que des segments scellés (append terminé + fsync).
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: le segment est scellé — aucune écriture concurrente possible.
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(Self { mmap })
    }

    /// Retourne une vue `&[u8]` sur la totalité du segment mappé.
    ///
    /// La durée de vie est liée à `&self` — le mapping reste valide tant que
    /// `MmapSegment` est vivant.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.mmap
    }

    /// Interprète les bytes du segment comme un slice continu de `T`.
    ///
    /// Un seul cast pour la totalité du mapping — aucun cursor, aucune copie
    /// intermédiaire. Les bytes en fin de segment qui ne forment pas un `T`
    /// complet sont ignorés silencieusement.
    ///
    /// # Safety
    ///
    /// L'appelant garantit que les bytes ont été écrits avec le layout de `T` :
    /// - même taille (`size_of::<T>()`)
    /// - même alignement (garanti car `mmap` est aligné sur une page de 4096 bytes)
    /// - pas de padding non-initialisé observable
    ///
    /// Dans Ashta-TS, `T = Event` (`repr(C)`, 40 bytes) et le segment a été
    /// produit par `SegmentWriter` — ces invariants sont garantis.
    #[inline]
    pub unsafe fn as_slice<T: Copy>(&self) -> &[T] {
        let elem_size = std::mem::size_of::<T>();
        if elem_size == 0 {
            return &[];
        }
        let len = self.mmap.len() / elem_size;
        // SAFETY: voir doc — appelant garantit layout + alignement.
        unsafe { std::slice::from_raw_parts(self.mmap.as_ptr() as *const T, len) }
    }

    /// Taille du segment en bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    /// Retourne `true` si le fichier mappé est vide (taille = 0).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_file(data: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join("ashta_mem_test.bin");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(data).unwrap();
        f.sync_all().unwrap();
        path
    }

    #[test]
    fn open_maps_correct_bytes() {
        let expected: Vec<u8> = (0u8..=255).collect();
        let path = write_temp_file(&expected);

        let seg = MmapSegment::open(&path).unwrap();
        assert_eq!(seg.len(), 256);
        assert_eq!(seg.as_bytes(), expected.as_slice());
    }

    #[test]
    fn open_nonexistent_returns_error() {
        let result = MmapSegment::open("/nonexistent/path/segment.alog");
        assert!(result.is_err());
    }

    #[test]
    fn empty_file_is_handled() {
        let path = write_temp_file(&[]);
        // memmap2 interdit le mmap d'un fichier vide sur certains OS —
        // on vérifie juste qu'on ne panique pas (erreur ou succès, les deux sont ok).
        let _ = MmapSegment::open(&path);
    }

    #[test]
    fn as_bytes_len_is_consistent() {
        let data = b"ashta-ts segment data";
        let path = write_temp_file(data);
        let seg = MmapSegment::open(&path).unwrap();
        assert_eq!(seg.as_bytes().len(), seg.len());
        assert!(!seg.is_empty());
    }
}
