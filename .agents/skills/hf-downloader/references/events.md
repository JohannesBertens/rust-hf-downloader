# NDJSON event schema — `download --json`

Stable, **additive-only** contract, pinned by insta snapshot tests. One JSON
object per line on **stdout**, flushed immediately. On failure the `error`
event is **always the last line**. Every event has `"type"` (snake_case tag).

On a resolution error (`ambiguous`, `no_files_match`, `usage`, `not_found`,
`network`), a single `error` event is the **only** stdout output.

## Event flow

```
resolved
├─ download_start          (per file)
│  ├─ progress*            (throttled to ≥500 ms apart)
│  └─ file_complete
├─ verification_start      (per verified file)
│  └─ verification_result
└─ done                    (always, unless a resolution error preempted the run)
```

## Events

### `resolved`

Emitted once, before queueing.

| Field | Type | Notes |
|---|---|---|
| `model` | string | the model ID |
| `files` | array of `FileDto` | everything selected |
| `total_bytes` | u64 | |

`FileDto`: `{ "filename": str, "size_bytes": u64, "sha256": str|null }`

### `download_start`

| Field | Type | Notes |
|---|---|---|
| `filename` | string | |
| `index` | usize | 1-based position within the run |
| `count` | usize | total files in the run |
| `size_bytes` | u64 | |

### `progress`

Throttled: at most one per 500 ms per run (the final tick before completion
may be dropped — rely on `file_complete`, not a 100 % progress).

| Field | Type | Notes |
|---|---|---|
| `filename` | string | currently-active file |
| `downloaded_bytes` | u64 | |
| `total_bytes` | u64 | |
| `speed_mbps` | f64 | MiB/s |
| `percent` | f64 | rounded to 0.1 |

### `file_complete`

| Field | Type | Notes |
|---|---|---|
| `filename` | string | |
| `status` | string | `"downloaded"` or `"already_exists"` |
| `bytes` | u64 | |

`already_exists` = skipped download, but the file is still re-verified.

### `verification_start` / `verification_result`

Emitted per file **with a known sha256** (LFS files; verification enabled by
default). Files whose hub entry has `sha256: null` (small non-LFS files such
as `config.json`) get **no** verification events and do not count in
`summary.verified`.

`verification_result`:

| Field | Type | Notes |
|---|---|---|
| `filename` | string | |
| `ok` | bool | |
| `expected_sha256` | string? | present iff `ok == false` |
| `actual_sha256` | string? | present iff `ok == false` |

### `done`

Always emitted on a run that got past resolution. On failure, `done` is
followed by trailing `error` events (one per failure category:
`download_failed`, `hash_mismatch`, `auth_required`, `interrupted`) — so on
failure the last stdout line is still an `error` event. Parse the `summary`
for counters; parse trailing `error` lines for causes.

| Field | Type | Notes |
|---|---|---|
| `summary.files` | usize | files in the run |
| `summary.downloaded` | usize | |
| `summary.skipped` | usize | already existed |
| `summary.verified` | usize | |
| `summary.failed` | usize | |
| `summary.hash_mismatch` | usize | |
| `summary.total_bytes` | u64 | |

### `error`

| Field | Type | Notes |
|---|---|---|
| `code` | string | see `cli-reference.md` for the code list |
| `message` | string | human-readable |
| `available` | array of `FileDto`? | only on `ambiguous` / `no_files_match` — the full candidate list |

## Search is NOT NDJSON

`search --json` emits **one JSON array document** (pretty-printed), not
events. NDJSON is reserved for the streaming download pipeline. On search
failure the only stdout output is a single `error` event line.

## Parsing recipes

```bash
# overall outcome
rust-hf-downloader download org/model --quant Q4_K_M --json | jq -c 'select(.type=="done")'

# live percent of active file
… | jq -c 'select(.type=="progress") | {f: .filename, pct: .percent}'

# candidates after an ambiguous probe (exit 64)
out=$(rust-hf-downloader download org/model --json) || code=$?
[ "${code:-0}" = 64 ] && echo "$out" | tail -1 | jq -r '.available[].filename'
```
