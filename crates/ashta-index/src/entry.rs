use ashta_core::symbol::SymbolId;

/// Entrée d'index pour une paire (symbol, segment).
///
/// Stocke les timestamps min/max observés pour un symbol donné dans un segment.
/// Permet de décider si un segment est pertinent pour une requête sans l'ouvrir.
///
/// Layout (40 octets, aligné 8 — symétrique avec Event) :
/// ┌──────────┬────────────┬────────┬──────────┬──────────┬─────────────┐
/// │  symbol  │ segment_id │  _pad  │  min_ts  │  max_ts  │ event_count │
/// │  8 bytes │  4 bytes   │ 4 bytes│  8 bytes │  8 bytes │   8 bytes   │
/// └──────────┴────────────┴────────┴──────────┴──────────┴─────────────┘
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IndexEntry {
    /// Identifiant du symbol concerné.
    pub symbol: SymbolId,

    /// Numéro du segment (`segment_0042.alog` → id = 42).
    pub segment_id: u32,

    /// Padding explicite pour aligner min_ts sur 8 bytes.
    pub _pad: [u8; 4],

    /// Timestamp minimum (ns) observé pour ce symbol dans ce segment.
    pub min_ts: u64,

    /// Timestamp maximum (ns) observé pour ce symbol dans ce segment.
    pub max_ts: u64,

    /// Nombre d'events de ce symbol dans ce segment.
    pub event_count: u64,
}

impl IndexEntry {
    /// Crée une entrée initiale avec un premier timestamp observé.
    pub fn new(symbol: SymbolId, segment_id: u32, first_ts: u64) -> Self {
        Self {
            symbol,
            segment_id,
            _pad: [0; 4],
            min_ts: first_ts,
            max_ts: first_ts,
            event_count: 1,
        }
    }

    /// Met à jour l'entrée avec un nouveau timestamp observé.
    ///
    /// Appelé à chaque event ingéré pour ce symbol dans ce segment.
    #[inline]
    pub fn observe(&mut self, ts: u64) {
        if ts < self.min_ts {
            self.min_ts = ts;
        }
        if ts > self.max_ts {
            self.max_ts = ts;
        }
        self.event_count += 1;
    }

    /// Indique si ce segment peut contenir des events dans `[t_start, t_end]`.
    ///
    /// Retourne `false` si on peut garantir l'absence — skip possible.
    /// Retourne `true` si le segment est *candidat* (il faut le lire pour confirmer).
    #[inline]
    pub fn overlaps(&self, t_start: u64, t_end: u64) -> bool {
        self.min_ts <= t_end && self.max_ts >= t_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_layout_is_stable() {
        assert_eq!(std::mem::size_of::<IndexEntry>(), 40);
        assert_eq!(std::mem::align_of::<IndexEntry>(), 8);
    }

    #[test]
    fn observe_updates_min_max() {
        let sym = SymbolId::from("BTC/USD");
        let mut e = IndexEntry::new(sym, 0, 5_000);

        e.observe(3_000); // nouveau min
        e.observe(8_000); // nouveau max
        e.observe(6_000); // entre les deux — pas de changement min/max

        assert_eq!(e.min_ts, 3_000);
        assert_eq!(e.max_ts, 8_000);
        assert_eq!(e.event_count, 4); // new + 3 observe
    }

    #[test]
    fn overlaps_logic() {
        let sym = SymbolId::from("ETH/USD");
        let mut e = IndexEntry::new(sym, 0, 10_000);
        e.observe(50_000);
        // e couvre [10_000, 50_000]

        assert!(e.overlaps(5_000, 15_000));   // début de la fenêtre dans le segment
        assert!(e.overlaps(20_000, 30_000));  // fenêtre entièrement dans le segment
        assert!(e.overlaps(40_000, 60_000));  // fin de la fenêtre dans le segment
        assert!(e.overlaps(1_000, 100_000));  // fenêtre englobe le segment
        assert!(!e.overlaps(60_000, 70_000)); // fenêtre entièrement après
        assert!(!e.overlaps(1_000, 9_000));   // fenêtre entièrement avant
    }
}
