# Release Notes - Version 2.7.0

**Release Date**: 2026-09-26

## New features

### Download from any branch, tag, or commit (`--revision`, issue #28)

Repos that keep their files on a non-`main` branch (e.g.
`NeuroSenko/MiniMax-M2.7-exl3`, all weights on the `2.0bpw` branch with an
empty `main`) were previously unfetchable: `main` was hardcoded in the file
tree listings, the resolve URLs, and the raw-endpoint fallback rewrite.

```bash
rust-hf-downloader download NeuroSenko/MiniMax-M2.7-exl3 --revision 2.0bpw --file model.gguf
```

- `--revision REV` accepts a git branch name (including slash-separated
  names like `release/1.0`), a tag, or a commit SHA; the file listing and
  every download URL follow the revision
- Revision values are validated at parse time (empty values, `..`
  traversal, edge slashes, and control characters are rejected)
- Registry entries record the revision (serde-defaulted — pre-v2.7.0
  registries keep loading unchanged); the TUI's resume flow re-downloads
  from the recorded revision instead of silently falling back to `main`
- Unknown revisions (and now unknown repos, see below) exit `64` with a
  `not_found` error event

### Rate-limit flags on the `download` subcommand (issue #26)

Rate limiting was config-file-only, which forced pipeline users (e.g.
Tekton) to ship a `config.toml`. The CLI can now control it per run:

```bash
rust-hf-downloader download org/model --all --rate-limit-mbps 25
```

- `--rate-limit-mbps MBPS` enables the limiter at the given rate (positive
  and finite; `0` is rejected — it would stall the transfer)
- `--rate-limit` enables limiting at the configured rate;
  `--no-rate-limit` disables it (both override the config file, and both
  conflict with each other)

## Fixes

- **Unknown repos/revisions now report `not_found`** instead of surfacing
  as a JSON-decode `network` error: the metadata and tree fetches call
  `error_for_status`, so HTTP 404s map to exit code `64` with a
  `not_found` error event (matching the CLI's existing exit-code table).
- The raw-endpoint fallback rewrite (`/resolve/{rev}/` → `/raw/{rev}/`)
  is revision-aware instead of hardcoded to `main`.

## Tests

- The e2e mock HuggingFace server is revision-aware (per-revision trees,
  empty-`main` layout, 404 for unknown revisions); three new e2e tests
  cover branch download, empty-`main` failure, and unknown-revision 404
- New unit tests: `resolve_url` revision embedding, `--revision` and
  rate-limit arg parsing, rate-limit config override precedence
