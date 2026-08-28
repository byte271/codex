# Context store

## Problem

Codex child spawn with `InitialHistory::Forked(Vec<RolloutItem>)` copies sanitized parent history into the child. Fan-out is O(children × retained parent bytes).

## Model

```
CAS
 ├── chunk[blake3] = bytes   (immutable)
 └── snapshot S = list of chunk hashes + optional parent snapshot id

Parent → S
Child A → S + delta A   (unchanged prefix hashes reused)
Child B → S + delta B
```

`ContentStore::snapshot_delta` compares chunk hashes at the same index and only writes new blobs when the bytes differ. Refs are counted in `refs.json`. `gc()` deletes unreferenced chunk files.

Reconstruction concatenates chunks and re-hashes each for corruption detection.

## Benchmark (VERIFIED, synthetic)

Parent blob: 256 KiB unique bytes (not a repeated byte — that would over-dedup). Chunk size 4096. Each child appends 1028 bytes.

Measured `unique_bytes` on 2026-08-27, this VM:

| Children | Naive copy (formula) | Shared formula | Measured unique | Amplification vs parent |
|---|---:|---:|---:|---:|
| 1 | 525,316 | 263,172 | 263,172 | 1.004 |
| 10 | 2,893,864 | 272,424 | 272,424 | 1.039 |
| 100 | 26,579,344 | 364,944 | 364,944 | 1.392 |

Naive 100-child copy is ~101× the parent. Shared is ~1.39×.

This is **not** a measurement of live Codex session directories. It is a fair synthetic of “copy parent + small delta” vs CAS. **LIKELY** the same ratio applies to forked rollouts dominated by shared prefix items.

Image-heavy / compacted histories: not separately measured. Compacted Codex already replaces a prefix with a summary; CAS still wins on the *shared suffix* and on binary chunks. **HYPOTHESIS.**

## Compatibility with Codex compaction

A `CompactedItem` can become a snapshot of the replacement history plus a chunk for the summary string. Child forks then share that snapshot id instead of cloning `Vec<RolloutItem>`. Adapter work; not wired into `codex-core`.
