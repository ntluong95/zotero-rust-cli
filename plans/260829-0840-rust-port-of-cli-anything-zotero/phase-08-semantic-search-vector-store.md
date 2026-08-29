---
phase: 8
title: "Semantic Search Vector Store"
status: todo
priority: P2
effort: "2-3d"
dependencies: [4]
---

# Phase 8: Semantic Search Vector Store

## Overview

Port `core/semantic.py` (259 LOC, 3 commands). Small, self-contained, and the **only place in the
entire codebase where Rust delivers a genuine CPU speedup**. Also fixes D2 (SQL injection).

Depends only on Phase 4 (SQLite), so it can run in parallel with Phases 5–7.

## Requirements

**Functional**
- `item build-index` — read items from `zotero.sqlite`, embed via the configured API, store f32 vectors
- `item semantic-search` — embed query, cosine-rank stored vectors
- `item similar` — rank against a stored item's vector
- Read and write the **existing** vector database format without migration

**Non-functional**
- Cosine ranking under 10 ms for 5,754 × 768 (Python baseline: 261 ms)
- Bit-compatible f32 blob encoding

## Architecture

```
crates/zotero-cli/src/
  semantic/
    mod.rs         # build_index, semantic_search, find_similar
    vectors.rs     # f32 blob encode/decode, cosine
    embed.rs       # OpenAI-compatible embeddings client
```

### Measured opportunity

Python computes cosine similarity in an interpreted loop over every stored vector
(`semantic.py:39-46`):

| Library × dim | Python | Rust (expected) |
|---|---|---|
| 1,000 × 768 | ~44 ms | <2 ms |
| 5,754 × 768 | ~261 ms | <5 ms |
| 5,754 × 1536 | ~580 ms | <10 ms |

Roughly **50×**. Honest framing: the operation is preceded by a network embedding call with a 10 s
timeout, so *end-to-end* improvement is partial — but for a warm local embedding server the ranking
step genuinely dominates.

A plain `f32` loop with iterators is sufficient; LLVM auto-vectorizes it. Do **not** reach for
explicit SIMD, `ndarray`, or a vector-database crate. That would add dependency weight and
cross-compilation risk for a 3-command feature.

### D2 fix — bound parameters

```python
# semantic.py:58-59 — SQL injection from CLI arguments
lang_filter = f"AND e.language = '{language}'" if language != "all" else ""
key_filter  = f"AND e.item_key != '{exclude_key}'" if exclude_key else ""
```

`--language` and the item key flow from the command line into SQL unescaped. Replace with bound
parameters and conditional SQL fragments carrying placeholders. Add a regression test using
`--language "x' OR '1'='1"`.

### Blob format compatibility

Python: `struct.pack(f"{len(vec)}f", *vec)` — native-endian f32 sequence. On all five target
platforms (x86_64 and aarch64) this is little-endian. Use `f32::to_le_bytes` and document that
big-endian hosts are unsupported, matching the effective status quo.

The vector DB path is `ZOTERO_VECTOR_DB`, defaulting to `~/Zotero/cli-anything-vectors.sqlite`.
Schema (`embeddings`, `vectors_f32`) must be created with identical `CREATE TABLE IF NOT EXISTS`
statements so an existing index remains usable and both implementations can share one database
during migration.

### Configuration

| Variable | Default |
|---|---|
| `ZOTERO_EMBED_API` | `http://127.0.0.1:8080/v1/embeddings` |
| `ZOTERO_EMBED_MODEL` | `nomic-embed-text` |
| `ZOTERO_EMBED_KEY` | `""` (Bearer header only when set) |
| `ZOTERO_VECTOR_DB` | `~/Zotero/cli-anything-vectors.sqlite` |

> Python reads these at **module import time** into globals. In Rust, read them per invocation —
> observably identical for a short-lived CLI, and it makes tests trivial.

### Behavioural details to preserve

- `_detect_language`: `zh` when CJK characters exceed 30% of the text, else `en`
- Score rounding: `round(score, 4)` — must match, since it is compared
- Dedupe by `item_key`, keeping the highest score, then take `top_k`
- `chunk_text` truncated to 200 chars in results, 2000 chars when stored
- `build_index` skips already-indexed keys and commits every `batch_size` (default 20)
- `build_index` excludes `itemTypeID NOT IN (1, 14)`

## Related Code Files

- Create: `src/semantic/mod.rs`, `vectors.rs`, `embed.rs`
- Create: `tests/semantic_vectors.rs`, `tests/semantic_sql_injection.rs`
- Reference: `core/semantic.py`

## Implementation Steps

1. Implement `vectors.rs`: little-endian f32 encode/decode plus cosine. Unit-test round-trip against
   a blob produced by Python `struct.pack`.
2. Implement `embed.rs` against the OpenAI embeddings response shape (`data[0].embedding`).
3. Implement `build_index` with the exact item query, language detection, skip logic and batching.
4. Implement `semantic_search` and `find_similar` with bound parameters (D2), rounding, dedupe and
   `top_k`.
5. Add the SQL-injection regression test.
6. Benchmark cosine ranking at 5,754 × 768 and assert under 10 ms.
7. Verify a Python-built vector DB is readable by Rust and vice versa.

## Success Criteria

- [ ] All 3 commands reach **Semantic** class (float ties may reorder; scores must match to 4 dp)
- [ ] Cosine ranking under 10 ms at 5,754 × 768 — a measured ≥25× improvement over the 261 ms baseline
- [ ] f32 blobs bit-identical to Python `struct.pack` output
- [ ] A Python-generated vector DB works unchanged under Rust, and vice versa
- [ ] `--language "x' OR '1'='1"` is treated as a literal value and returns no rows — no SQL error, no injection
- [ ] Language detection matches Python on a mixed CJK/Latin corpus
- [ ] `build_index` skip/batch/commit behaviour matches
- [ ] No new heavyweight dependency (`ndarray`, SIMD crates, vector DBs) added

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Float summation order changes scores at the 4th decimal | Sum in the same order as Python (sequential, not chunked); assert to 4 dp |
| Tie ordering differs and reshuffles results | Classified **Semantic**, not Exact; add a deterministic secondary sort on `item_key` |
| Endianness assumption wrong on some future target | Documented; all five current targets are little-endian; assert at build time |
| Embedding endpoint unavailable in CI | Fake embeddings server in the test harness |
| Shared DB corrupted by concurrent Python and Rust writes during migration | Document that `build-index` should not be run concurrently from both implementations |
