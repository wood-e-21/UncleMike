# Uncle Mike

> Sovereign local AI legal platform. Rust backend + Electron shell + Python sidecars + Next.js frontend + Microsoft Word add-in + MCP server. All cite-able content lives on disk as Markdown.

**Status:** pre-alpha. See [`PLAN.md`](PLAN.md) for the build plan, [`docs/`](docs/) for contracts.

## Lineage

Uncle Mike descends from open-source legal-AI projects, in order of architectural influence:

- [`SemplificaAI/MikeRust`](https://github.com/SemplificaAI/MikeRust) — Rust data layer, sqlite-vec partition keys, hash-keyed cache, chunker design, three-tier scope model. The trunk we forked.
- [`ebubekirkupe/mike`](https://github.com/ebubekirkupe/mike) — shared-types architecture, SSE event bus, Word add-in, MCP server pattern.
- [`willchen96/mike`](https://github.com/willchen96/mike) — the original Mike, parent of both forks above.

Internal codebase identifier remains `mike` (Cargo crate name, binary name, file extensions) for compatibility with the lineage. "Uncle Mike" is the product name.

License: AGPL-3.0-only (inherited).

## Quick start

Not yet runnable. Phase 0 (specs + skeleton) is in progress. When Phase 1 lands, `just dev` will bring up the full stack.

## Repo layout

```
.
├── PLAN.md                ← master build plan
├── docs/                  ← contracts (read 00 first)
├── electron/              ← Electron shell (TS) — Phase 1
├── backend/               ← Rust trunk (axum + sqlite-vec) — Phase 1
├── sidecars/              ← Python sidecars (Docling, eyecite) — Phase 3
├── frontend/              ← Next.js — Phase 1
├── word-addin/            ← Office add-in — Phase 5
├── packages/shared/       ← polyglot type hub (auto-generated)
├── scripts/               ← codegen + packaging
└── tests/                 ← cross-language E2E
```

Until Phase 1 fully strips the upstream Tauri/EUR-Lex code, the repo also contains:
- `src/` — current Rust backend (MikeRust-inherited)
- `src-tauri/` — Tauri shell (will be replaced by `electron/` in Phase 1)
- `migrations/` — current SQLite migrations (`backend/migrations/` will replace this)
- `frontend/` — current Next.js (will be reorganized but kept in Phase 1)

## Contributing

Read [`docs/00-anti-patterns.md`](docs/00-anti-patterns.md) before opening a PR. Every PR uses the template at [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md).
