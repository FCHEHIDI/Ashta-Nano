use crate::symbol::SymbolId;

/// Type de l'événement de marché.
/// u8 pour occuper 1 octet — le padding explicite gère l'alignement.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Trade     = 0,
    BidUpdate = 1,
    AskUpdate = 2,
}

/// Unité fondamentale d'Ashta-TS.
///
/// Invariants :
/// - timestamp_ns croissant par symbol (garanti par ashta-log, pas ici)
/// - layout fixe : repr(C), taille = 40 octets
/// - immuable une fois sérialisé sur disque
///
/// Layout mémoire (40 octets, aligné 8) :
/// ┌──────────────┬──────────┬──────────┬──────────┬─────────┬─────────┐
/// │ timestamp_ns │  symbol  │  price   │  volume  │  kind   │  _pad   │
/// │   8 octets   │ 8 octets │ 8 octets │ 8 octets │ 1 octet │ 7 octets│
/// └──────────────┴──────────┴──────────┴──────────┴─────────┴─────────┘
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Event {
    /// Nanosecondes depuis l'époque Unix (UTC).
    pub timestamp_ns: u64,

    /// Identifiant de l'instrument — 8 octets fixes, UTF-8, padé avec `\0`.
    /// repr(transparent) garantit le même layout que [u8; 8].
    pub symbol: SymbolId,

    /// Prix en f64.
    /// Note : en production HFT réelle on utilise du fixed-point (i64 * 10^-8).
    /// On commence par f64 pour la clarté — on discutera le tradeoff plus tard.
    pub price: f64,

    /// Volume échangé ou taille de l'ordre.
    pub volume: f64,

    /// Type d'événement.
    pub kind: EventKind,

    /// Padding explicite pour aligner la struct à 8 octets.
    /// TOUJOURS explicite — jamais laisser le compilateur décider silencieusement.
    pub _pad: [u8; 7],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_layout_is_stable() {
        // I3 : taille connue et fixe à la compilation
        assert_eq!(std::mem::size_of::<Event>(), 40);

        // Alignement sur 8 octets (compatible avec mmap et I/O direct)
        assert_eq!(std::mem::align_of::<Event>(), 8);
    }
}
