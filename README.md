<div align="center">
  <img src="liquid_topology.png" alt="Ashta-TS" width="100%"/>
</div>

<div align="center">

# Ashta-TS

**HFT time-series storage engine — Rust, append-only, zero heap on hot path**

![Rust](https://img.shields.io/badge/Rust-1.87-orange?style=flat-square&logo=rust)
![Edition](https://img.shields.io/badge/Edition-2024-orange?style=flat-square)
![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)
![Tests](https://img.shields.io/badge/Tests-52%20passing-brightgreen?style=flat-square)
![Status](https://img.shields.io/badge/Status-active-brightgreen?style=flat-square)

</div>

---

## Architecture

```
ashta-core     →  Event (40B, repr(C)) · SymbolId (8B, stack-only)
ashta-log      →  append-only segmented log · SegmentWriter/Reader · rotate + fsync
ashta-index    →  zone map (SymbolId × segment_id → ts_min/ts_max) · segment pruning
ashta-ingest   →  Binance CSV batch ingestion · nanosecond timestamps
ashta-query    →  read_range(symbol, t_start, t_end) · zone map pruning + sequential scan
ashta-replay   →  deterministic ordered streaming · VecDeque<SegmentReader> · backtest-ready
ashta-observe  →  lock-free metrics · LatencyHistogram 64 buckets · Probe RAII · p50/p90/p99
```

## Event layout

```
[timestamp_ns: u64][symbol: [u8;8]][price: f64][volume: f64][kind: u8][_pad: 7B]
 └── 8B             └── 8B          └── 8B       └── 8B       └── 1B   └── 7B = 40B total
```

`repr(C)` · 8-byte aligned · cast direct `&Event → &[u8]` sans sérialisation

## Crates

| Crate | Rôle | Tests |
|---|---|---|
| `ashta-core` | types fondamentaux | 6 |
| `ashta-log` | écriture segmentée | 7 |
| `ashta-index` | zone map + pruning | 6 |
| `ashta-ingest` | CSV Binance → log | 7 |
| `ashta-query` | range query | 4 |
| `ashta-replay` | streaming ordonné | 8 |
| `ashta-observe` | métriques lock-free | 16 |

## Quick start

```bash
cargo test --workspace   # 52 tests, 0 failed
```

## Roadmap

- [x] Segmented log + zone map index
- [x] Binance CSV ingestion
- [x] Range query + deterministic replay
- [x] Lock-free metrics (p50/p90/p99)
- [ ] `ashta-mem` — mmap SegmentReader (hugepages, NUMA-aware)
- [ ] `ashta-kernel` — PREEMPT_RT · CPU pinning · io_uring
