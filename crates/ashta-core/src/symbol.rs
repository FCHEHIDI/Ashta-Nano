/// Identifiant compact d'un instrument financier.
///
/// Encodé sur 8 octets fixes, padé avec `\0`. Pas d'allocation heap — vit sur la pile.
/// `repr(transparent)` garantit un layout identique à `[u8; 8]`.
///
/// Règles d'encodage :
/// - Tronqué silencieusement à 8 **bytes** (pas chars — un char Unicode peut faire 2-4 bytes)
/// - Padé à droite avec `\0`
///
/// Exemples :
/// - `"BTC/USD"` → `[B, T, C, /, U, S, D, \0]`
/// - `"AAPL"`    → `[A, A, P, L, \0, \0, \0, \0]`
/// - `"TOOLONG!"` → `[T, O, O, L, O, N, G, !]`  (troncature silencieuse)
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId([u8; 8]);

impl SymbolId {
    /// Crée un `SymbolId` directement depuis un tableau brut.
    /// Usage interne — désérialisation depuis disque.
    #[inline]
    pub const fn from_raw(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// Retourne le tableau brut sous-jacent.
    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    /// Décode en `&str` en strippant le padding `\0`.
    /// Retourne `""` si les bytes ne sont pas de l'UTF-8 valide.
    #[inline]
    pub fn as_str(&self) -> &str {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(8);
        std::str::from_utf8(&self.0[..end]).unwrap_or("")
    }
}

impl From<&str> for SymbolId {
    /// Encode une `&str` en `SymbolId`.
    ///
    /// - Tronque silencieusement à 8 bytes si la chaîne est trop longue.
    /// - Pade avec `\0` si elle est plus courte.
    fn from(s: &str) -> Self {
        let mut bytes = [0u8; 8];
        let src = s.as_bytes();
        let len = src.len().min(8);
        bytes[..len].copy_from_slice(&src[..len]);
        Self(bytes)
    }
}

impl std::fmt::Debug for SymbolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SymbolId(\"{}\")", self.as_str())
    }
}

impl std::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_layout_is_transparent() {
        // repr(transparent) — même taille que [u8; 8]
        assert_eq!(std::mem::size_of::<SymbolId>(), 8);
        assert_eq!(std::mem::align_of::<SymbolId>(), 1);
    }

    #[test]
    fn symbol_encode_decode_roundtrip() {
        let id = SymbolId::from("BTC/USD");
        assert_eq!(id.as_str(), "BTC/USD");
        assert_eq!(id.as_bytes(), b"BTC/USD\0");
    }

    #[test]
    fn symbol_short_is_padded() {
        let id = SymbolId::from("AAPL");
        assert_eq!(id.as_bytes(), b"AAPL\0\0\0\0");
        assert_eq!(id.as_str(), "AAPL");
    }

    #[test]
    fn symbol_too_long_is_truncated() {
        let id = SymbolId::from("TOOLONG!X");  // 9 chars → tronqué à 8
        assert_eq!(id.as_bytes(), b"TOOLONG!");
        assert_eq!(id.as_str(), "TOOLONG!");
    }

    #[test]
    fn symbol_equality_and_hash_work() {
        use std::collections::HashMap;

        let a = SymbolId::from("ETH/USD");
        let b = SymbolId::from("ETH/USD");
        let c = SymbolId::from("BTC/USD");

        assert_eq!(a, b);
        assert_ne!(a, c);

        let mut map: HashMap<SymbolId, u64> = HashMap::new();
        map.insert(a, 42);
        assert_eq!(map[&b], 42);  // b == a, même hash
    }
}
