# docs/09-performance.md

> Performance targets. Tracked in CI; regressions investigated.

## Philosophy

Performance targets are not v1 acceptance gates. They are tracked metrics. When something regresses by more than 20% from the previous release, we look. Anything worse than 2× the target is a bug.

The targets below are for a **reference workspace** on **reference hardware**.

### Reference hardware
- Apple M2 Pro (16 GB RAM, 1 TB SSD)
- macOS 14+

### Reference workspace
- 5 matters
- 200 items per matter (~1,000 total: 700 emails, 200 documents, 70 notes, 30 appointments)
- 50 attachments per matter (~250 total, average 2 MB each)
- Average document: 12 pages
- Average email: 800 words, 1 attachment

## Targets

### Cold start

| Operation | Target (p95) |
|---|---|
| Electron launch → lock screen visible | < 1.5 s |
| PIN entry → frontend interactive | < 2.5 s |
| (sub-step) backend spawn → READY | < 800 ms |
| (sub-step) sidecar spawn (Docling) | < 4 s |

Sidecar startup is dominated by Python interpreter + model load. We don't block the frontend on sidecars — they spin up in the background; routes that need them return 503 until ready.

### Search

| Operation | Target (p95) |
|---|---|
| Hybrid search (FTS5 + vec0), 1k items | < 100 ms |
| Hybrid search, 10k items | < 300 ms |
| Hybrid search, 100k items | < 1 s |
| Item detail open (read .md + parse frontmatter) | < 50 ms |
| Filter by matter (single matter, 200 items) | < 30 ms |

### Ingestion

| Operation | Target (median) |
|---|---|
| Upload PDF (20 pages) → searchable | < 8 s |
| (sub-step) Docling parse | < 5 s |
| (sub-step) chunk + embed (12 chunks) | < 2 s |
| Upload PDF, 100 pages | < 30 s |
| Initial Gmail sync, last 30 days, 500 emails | < 10 min |
| Initial IMAP sync, 1k messages | < 15 min |

### Rebuild

| Operation | Target |
|---|---|
| Foreground rebuild, 1k items | < 30 s |
| Foreground rebuild, 10k items | < 5 min |
| Background embedding catch-up, 10k items | < 30 min |

### Cross-surface latency

| Operation | Target (p95) |
|---|---|
| SSE event from publish to Word add-in DOM update | < 200 ms |
| SSE event from publish to frontend tab update | < 150 ms |
| Lock → SSE reconnect after backend restart | < 5 s |

### Chat

| Operation | Target |
|---|---|
| User send → first token from LLM (Claude Sonnet) | < 1.5 s (network-bound) |
| Citation extraction + render after stream ends | < 200 ms |
| Workflow apply (system-prompt injection + first token) | < 2 s |

LLM latency is mostly provider-bound. We measure to detect *our* overhead — if first-token is > 2× the provider's typical latency, something's wrong in our pipeline.

### Memory

| Process | Target idle RSS | Target peak RSS |
|---|---|---|
| Rust backend | < 80 MB | < 400 MB |
| Docling sidecar (1 worker) | < 1.5 GB | < 2.5 GB |
| eyecite sidecar (1 worker) | < 250 MB | < 400 MB |
| Electron main | < 150 MB | < 250 MB |
| Frontend renderer (Chromium) | < 250 MB | < 500 MB |
| Word add-in (in Word's WKWebView) | < 100 MB | < 200 MB |
| **Total Mike footprint (idle)** | **< 2.5 GB** | — |
| **Total Mike footprint (active)** | — | **< 4.5 GB** |

For comparison: a single Chrome tab with a complex web app is ~300 MB. Mike should not be obnoxious relative to other modern desktop apps.

### Storage

| Metric | Target |
|---|---|
| `<workspace>/.mike/` overhead per item | < 5 KB |
| `<workspace>/.mike/sidecar-cache/` size | < 500 MB (post first run) |
| SQLite file size for 10k items | < 200 MB (with embeddings) |

## Measurement

### CI benchmark suite

`tests/bench/` contains:
- `bench_search.rs` — runs search queries against a fixture workspace, measures p50/p95/p99.
- `bench_ingest.rs` — uploads fixture PDFs, measures end-to-end ingest latency.
- `bench_rebuild.rs` — rebuilds a fixture workspace from `.md`, measures foreground duration.
- `bench_startup.sh` — times cold start with a fixture workspace, parsing log timestamps.

Run as `just bench`. CI runs them nightly on the reference platform; results posted to a tracking sheet.

### Production telemetry

None. We don't ship telemetry (Decision 12 in security model). Users who hit performance issues report them via the support email, and we attempt to repro on the reference platform.

## Regression policy

For every release:
1. Run `just bench` against the new build.
2. Compare to the previous release's numbers.
3. Any metric > 20% worse: investigate before tagging the release.
4. Any metric > 2× target: ship-blocking bug.

## Improvement targets

Areas where we know there's room (and a release that focuses on them):

1. **Cold start.** Docling sidecar takes ~4 s. PyInstaller-bundled Python is slow; switching to Nuitka or Cython-compiled bundles could halve it.
2. **Embedding.** Currently CPU-only fastembed. Adding CoreML execution provider would cut by 3-5× on Apple Silicon.
3. **Search.** Hybrid search currently does FTS5 and vec0 separately, merges in Rust. A single query with sqlite-vec's `KNN MATCH` + FTS5 join could halve latency.
4. **Frontend bundle size.** Next.js standalone output is ~5 MB gzipped; could likely be 2 MB with aggressive code-splitting.

None of these is in alpha scope. Slot into the first stability release.

## What we don't optimize for

- **Multi-user throughput.** Mike is single-user. We never tune for "1000 concurrent users."
- **High-frequency search.** No sub-10ms targets. Lawyers don't type fast enough to need it.
- **Memory below 1 GB.** Modern lawyer laptops have 16+ GB. Not where the budget should go.
- **Network throughput.** All meaningful traffic is loopback. Network is bounded by external providers (LLM, mail, calendar).

What we DO optimize for: **first-time-user experience.** Cold start, first PDF ingest, first search — these get the most attention. After the first session, users adapt to whatever Mike does.
