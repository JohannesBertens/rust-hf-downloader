# CLI reference — rust-hf-downloader 2.3.0+

Both subcommands accept `--token TOKEN` (precedence: `--token` > `$HF_TOKEN`
> config file) and honor `HF_ENDPOINT` (base-URL override for mirrors or
local test servers).

## `download` (alias: `dl`)

```
rust-hf-downloader download [OPTIONS] <MODEL_ID>
```

`MODEL_ID` must be exactly `author/name` with non-empty parts. The config
file is optional: with no config, built-in defaults apply (`~/models`
output, download-count sort, etc.) — a fresh `$HOME` works out of the box.

| Flag | Value | Meaning |
|---|---|---|
| `--quant` | `TYPE` | All files of one GGUF quantization (e.g. `Q4_K_M`, `Q8_0`). Case-insensitive; includes **every part** of multipart archives |
| `--file` | `PATH` | Exact repo-relative file path. **Repeatable**; duplicates deduped, order preserved |
| `--all` | — | Entire repository (directories and size-less entries skipped) |
| `-o`, `--output` | `DIR` | Base output directory (default: config `default_directory`, usually `~/models`) |
| `--token` | `TOKEN` | HuggingFace token (gated repos need one) |
| `--no-verify` | — | Skip SHA256 verification |
| `--json` | — | NDJSON events on stdout (see `events.md`) |
| `-q`, `--quiet` | — | Human mode only: suppress progress; errors and final summary remain |

**Selector rules** — at most one of `--quant` / `--file` / `--all`.
Combining them exits `64` (usage). No selector works only when the repo has
exactly one downloadable file; otherwise exit `64` with the full file list
in `error.available`.

## `search`

```
rust-hf-downloader search <QUERY> [OPTIONS]
```

| Flag | Value | Meaning |
|---|---|---|
| `--sort` | `downloads`\|`likes`\|`modified`\|`name` | Sort field (default: config `default_sort_field`) |
| `--direction` | `asc`\|`desc` (aliases of ascending/descending) | Sort direction (default: config) |
| `--min-downloads` | `N` | Client-side filter |
| `--min-likes` | `N` | Client-side filter |
| `--limit` | `1..=500` (default `100`) | Max results |
| `--token` | `TOKEN` | As above |
| `--json` | — | One JSON **array** document on stdout (not NDJSON) |

Unspecified flags fall back to the TUI's config defaults. The full-text
`QUERY` is matched server-side by HuggingFace; `--min-*` filters,
`--sort name`, and ascending sorts apply client-side (the API only sorts
descending). Zero results: exit `0` with `[]`.

Human-mode output: fixed-column table on stdout, result count on **stderr**
(so the table stays pipeable).

## Exit codes

| Code | Constant | Meaning |
|---|---|---|
| 0 | `EXIT_OK` | All requested files present on disk (downloaded or already existed); verification passed or skipped |
| 1 | `EXIT_FAILURE` | Download failed after retries, or any hash mismatch |
| 2 | `EXIT_AUTH` | Authentication required (gated repo / bad token). clap usage errors are rerouted to 64 so `2` stays reserved for auth |
| 64 | `EXIT_USAGE` | Usage error, resolution ambiguity, unknown file, model not found (`not_found`), or a clap parse error |
| 130 | `EXIT_INTERRUPTED` | SIGINT; unfinished files stay registered as incomplete and restart from scratch on the next run |

## Error event codes (`{"type":"error","code":…}`)

`usage`, `not_found`, `network`, `ambiguous`, `no_files_match`,
`invalid_path`, `download_failed`, `hash_mismatch`, `auth_required`,
`verification_error`, `interrupted`, `internal`.

Only `ambiguous` and `no_files_match` carry `available` (the structured
file list — see `events.md`).

## Files and registry

- Destination layout: `<output>/<author>/<model-name>/<repo-relative-path>`
  (same layout as the TUI).
- Registry: `~/models/hf-downloads.toml` — shared with the TUI, so CLI
  downloads appear in the TUI's resume/complete views and vice versa.
- Interrupted files are recorded as incomplete; the next run restarts them
  from scratch (no byte-range resume across runs).
