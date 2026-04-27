# Charte Ashta-TS

---

## 1. Intention

Ashta-TS est un moteur de base de données time-series spécialisé HFT-like, écrit en **Rust**, conçu pour :

- Ingérer des flux de données haute fréquence (ticks, ordres, événements, signaux)
- Stocker ces données en append-only avec un layout mémoire contrôlé
- Servir des lectures par fenêtre temporelle et par instrument avec une latence stable
- Permettre le replay déterministe pour backtests et simulations

Ashta-TS est d'abord **un outil pour son auteur** : un système artisanal, compréhensible de bout en bout, aligné sur des besoins réels de recherche, d'expérimentation et de démonstration.

---

## 2. Ce qu'Ashta-TS est

### Un moteur spécialisé, pas généraliste
- Optimisé pour des séries temporelles haute fréquence
- Applicable à plusieurs domaines : HFT, IA/ML, médical, IoT, HPC
- Pensé pour un environnement co-designé (kernel tuné, réseau optimisé, NUMA)

### Un système byte-oriented
- Layout mémoire explicite et contrôlé
- Formats binaires simples, stables, documentés
- Usage réfléchi de `mmap`, buffers, cache, alignement

### Un log segmenté append-only
- Écriture séquentielle
- Segments rotatifs (par taille ou par temps)
- Index minimaliste pour retrouver rapidement les fenêtres pertinentes

### Un moteur Rust-native
- Sécurité mémoire sans GC
- Contrôle total sur le layout et les abstractions
- Performance prévisible, pas "espérée"

### Un laboratoire d'architecture
- Terrain d'exploration : invariants, layout, caches, index, NUMA
- Support pédagogique pour comprendre un storage engine moderne de bas en haut

---

## 3. Ce qu'Ashta-TS n'est pas (et ne sera pas)

| Ce qu'on évite         | Pourquoi                                              |
|------------------------|-------------------------------------------------------|
| RDBMS généraliste      | Pas de SQL complet, pas de joins arbitraires          |
| Clone de l'existant    | On emprunte des idées (LSM, WAL, mmap), pas des archi |
| Solution enterprise    | Pas de clustering magique, pas de HA out-of-the-box   |
| Produit marketing      | Ashta assume sa spécialisation et ses limites         |
| Transactions ACID multi-tables | Hors périmètre                              |

---

## 4. Les 8 modules

| Module          | Rôle                                                       |
|-----------------|------------------------------------------------------------|
| `ashta-core`    | Primitives binaires, layout, sérialisation, types de base  |
| `ashta-log`     | Append-only segmented log                                  |
| `ashta-index`   | Index temporel / symbol / clé                              |
| `ashta-mem`     | Cache, mmap, pages, buffers                                |
| `ashta-query`   | Moteur de requêtes minimal (`read_range`, filtres)         |
| `ashta-ingest`  | Ingestion haute fréquence                                  |
| `ashta-replay`  | Lecture ordonnée, backtest, streaming                      |
| `ashta-observe` | Métriques, traces, introspection                           |

---

## 5. Invariants non négociables

1. **Ordre temporel par instrument** — pour un symbol donné, les événements sont stockés et relus dans un ordre total cohérent (timestamp + tie-break)
2. **Append-only** — pas de mise à jour in-place ; les corrections passent par de nouveaux événements (event sourcing)
3. **Lisibilité de l'architecture** — chaque module s'explique en quelques phrases, pas de magie cachée
4. **Crash safety explicite** — les garanties de durabilité sont documentées (`fsync`, modes de flush), pas implicites
5. **Mesurabilité** — latence, throughput, cache hit ratio sont observables ; on mesure, on ne suppose pas

---

## 6. Métriques qui comptent

- Latence de lecture d'une fenêtre `[t_start, t_end]` pour un symbol
- Latence d'ingestion (amortie) pour un flux continu
- **Stabilité** de la latence (jitter faible > latence minimale absolue)
- Footprint mémoire pour un volume donné
- Taille et structure des segments (efficacité du layout)

---

## 7. Ouverture

Ashta-TS est ouvert à :
- L'inspiration d'autres domaines (IA, IoT, médical, HPC)
- L'usage par d'autres, si leurs contraintes s'alignent
- L'extension (modules, plugins, formats) — tant que les invariants de base sont respectés

**Priorité absolue** : servir à 100% les besoins de son auteur, avec une compréhension totale du système.

---

## 8. Philosophie de développement

- **Pas de précipitation** — projet de longue haleine, chaque couche est comprise avant d'être construite
- **Apprendre d'abord** — théorie → invariants → code, couche par couche
- **Exploration scientifique** — on part de zéro, on teste des hypothèses, on mesure, on itère
- **Backend avant kernel** — le storage engine Rust d'abord ; le kernel tuning vient en renfort, pas en prérequis
