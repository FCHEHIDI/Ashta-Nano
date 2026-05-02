# Composants CPU & Mémoire — Ashta-TS / mmap

## Vue d'ensemble du matériel impliqué

```
┌─────────────────────────────────────────────────────────────────┐
│                          CPU (core HFT)                         │
│                                                                 │
│   ┌──────────┐    ┌──────────┐    ┌──────────────────────────┐  │
│   │  L1$     │    │  L2$     │    │  TLB                     │  │
│   │  32 KB   │◄──►│  256 KB  │    │  (adresse virtuelle      │  │
│   │  ~1 ns   │    │  ~4 ns   │    │   → adresse physique)    │  │
│   └──────────┘    └──────────┘    │  miss → page table walk  │  │
│         ▲               ▲         └──────────────────────────┘  │
│         └───────────────┘                      ▲                │
│                   ▲                             │                │
└───────────────────┼─────────────────────────────────────────────┘
                    │                             │
        ┌───────────▼─────────────┐   ┌───────────▼──────────────┐
        │  L3$ (partagé)          │   │  Page Table              │
        │  8–32 MB                │   │  (en RAM)                │
        │  ~10 ns                 │   │  adresse virtuelle       │
        └───────────┬─────────────┘   │  → frame physique        │
                    │                 └──────────────────────────┘
                    │
        ┌───────────▼─────────────────────────────────────────────┐
        │                  RAM (DRAM)                             │
        │                  DDR5 — 64 GB                           │
        │                  ~60–80 ns                              │
        │                                                         │
        │   ┌─────────────────────┐  ┌──────────────────────┐    │
        │   │  Heap process       │  │  Page cache kernel   │    │
        │   │  (allocations Rust) │  │  (fichiers mappés)   │    │
        │   │                     │  │                      │    │
        │   │  Vec<u8> ← fs::read │  │  segment_0000.alog   │    │
        │   │  (copie ici)        │  │  (pages physiques)   │    │
        │   └─────────────────────┘  └──────────────────────┘    │
        └──────────────────────────────────┬──────────────────────┘
                                           │
        ┌──────────────────────────────────▼──────────────────────┐
        │                  Stockage NVMe                          │
        │                  ~50–100 µs (cold)                      │
        │                                                         │
        │              segment_0000.alog (sur disque)             │
        └─────────────────────────────────────────────────────────┘
```

---

## Chemin d'un accès — `fs::read()` (ancien code)

```
CPU veut bytes[42]
      │
      ▼
  L1$ miss → L2$ miss → L3$ miss
      │
      ▼
  RAM — heap process (Vec<u8>)           ← lecture ici
      ▲
      │  copie 2 (kernel → heap)
      │
  RAM — page cache kernel
      ▲
      │  copie 1 (disque → page cache)
      │
  NVMe — segment_0000.alog

  Bilan : 2 copies RAM, heap allouée = taille segment entier
```

---

## Chemin d'un accès — `mmap` (code actuel)

```
CPU veut bytes[42]
      │
      ▼
  TLB lookup : adresse virtuelle 0x7f3a00002a → frame physique ?
      │
      ├── TLB hit  →  adresse physique directe → L1$/L2$/L3$/RAM
      │               (cas normal après premier accès, ~1 ns)
      │
      └── TLB miss →  page table walk (RAM)
                          │
                          ├── page présente  →  mappe dans TLB → L1$
                          │
                          └── page absente   →  PAGE FAULT
                                │
                                ▼
                            Kernel charge la page (4 KB) depuis NVMe
                            → page cache RAM
                            → mappe dans page table
                            → retour au CPU

  Bilan : 1 seule copie (disque → page cache)
          la heap process n'est PAS allouée
          pages non accédées = jamais lues (demand paging)
```

---

## Impact des hugepages (objectif `ashta-kernel`)

```
                    Pages 4 KB (défaut)        Hugepages 2 MB
                    ─────────────────          ──────────────

  segment 64 MB  →  16 384 entrées TLB    →   32 entrées TLB
  TLB capacity   →  saturé rapidement     →   confortable
  TLB miss rate  →  élevé                 →   quasi zéro
  coût/accès     →  ~10–40 ns (miss)      →   ~1 ns (hit)

  Pour Ashta-TS : scan séquentiel d'un segment entier
  → avec 4KB : TLB se vide et se recharge en boucle
  → avec 2MB : TLB tient tout le segment, zéro eviction
```

---

## Résumé des ordres de grandeur

```
  L1 cache hit          1 ns
  L2 cache hit          4 ns
  L3 cache hit         10 ns
  RAM (TLB hit)        60 ns
  RAM (TLB miss)      100–200 ns   ← page table walk
  NVMe (page cache)    27 µs       ← mmap_open mesuré
  NVMe (cold read)    100 µs       ← première lecture d'une page absente

  ashta-bench baseline (page cache chaud, 100k events) :
    read_sequential  →  1.3 ms  →  79 Mevents/s
    mmap_open        →   27 µs  →  syscall seul, zéro I/O disque
```
