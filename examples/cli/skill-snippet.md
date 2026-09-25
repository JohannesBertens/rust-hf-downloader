# AI-skill usage: one-shot model download

> **This snippet is now a real installable skill.** The full Agent Skills
> version — with routing, an NDJSON event reference, and a ready-made
> probe→select→download wrapper — lives in
> [`.agents/skills/hf-downloader/`](../.agents/skills/hf-downloader/SKILL.md)
> (see the README's "Use as an agent skill" section for installation).
> This file remains as a compact human-readable example.

`rust-hf-downloader download` is designed to be driven by scripts and agent
skills. Together with `search`, the whole flow stays inside the binary:

1. **Discover the model ID.**

   ```bash
   $ rust-hf-downloader search "qwen 2.5 gguf" --limit 5 --json
   [
     {
       "id": "bartowski/Qwen2.5-7B-GGUF",
       "downloads": 172000,
       ...
     },
     ...
   ]
   ```

   Queries emit a single JSON array (not NDJSON); filters/sort flags mirror
   the TUI: `--sort downloads|likes|modified|name`, `--direction asc|desc`,
   `--min-downloads N`, `--min-likes N`. Zero results is still exit `0`.

2. **Start with a probe.** Invoke `download` with no selector on purpose when the repo
   contents are unknown — an ambiguous repo returns exit code `64` plus a
   structured file list you can choose from:

   ```bash
   $ rust-hf-downloader download bartowski/Qwen2.5-7B-GGUF --json
   {"type":"error","code":"ambiguous","message":"model has 9 downloadable file(s); ...","available":[{"filename":"Qwen2.5-7B-Q4_K_M.gguf","size_bytes":4947802324,"sha256":"..."}, ...]}
   $ echo $?
   64
   ```

2. **Re-invoke with the selector** picked from `available` (or `--quant` for a
   quantization, `--all` for everything):

   ```bash
   rust-hf-downloader download bartowski/Qwen2.5-7B-GGUF --quant Q4_K_M --json
   ```

3. **Read NDJSON events from stdout** (progress on stdout too, throttled to
   500 ms; the `error` event is always the last line on failure):

   ```bash
   rust-hf-downloader download org/model --file model.gguf --json \
     | jq -c 'select(.type == "progress") | {pct: .percent, eta: .speed_mbps}'
   ```

4. **Decide from the exit code:**

   | Code | Meaning | Skill action |
   |---|---|---|
   | 0 | files present, verified/skipped | proceed |
   | 1 | download failed or hash mismatch | inspect final `error` event, surface to user |
   | 2 | auth required | request token, re-invoke with `--token` or `$HF_TOKEN` |
   | 64 | usage / unknown file / ambiguous | pick selector from `error.available`, retry once |
   | 130 | interrupted | optionally retry (restarts from scratch) |

5. **Idempotency:** files that already exist are skipped (`file_complete`
   with `"status":"already_exists"`) and still re-verified, so re-running the
   same command is a cheap consistency check.

Minimal skill loop (bash):

```bash
set -euo pipefail
MODEL=$(rust-hf-downloader search "$QUERY" --limit 1 --json | jq -r '.[0].id')
out=$(rust-hf-downloader download "$MODEL" --json) || code=$?
code=${code:-0}
if [ "$code" = 64 ]; then
  file=$(echo "$out" | tail -1 | jq -r '.available[0].filename')
  exec rust-hf-downloader download "$MODEL" --file "$file" --json
fi
echo "$out" | tail -1 | jq .
exit "$code"
```

Notes:

- `HF_ENDPOINT` can point the tool at a mirror (e.g. `https://hf-mirror.com`).
- Human mode (no `--json`) writes progress to **stderr** and the summary to
  **stdout**, so `… 2>/dev/null` keeps logs clean.
- Downloads land in `<output>/author/model-name/…` (default `~/models`) and
  are tracked in the same registry the TUI uses (`~/models/hf-downloads.toml`).
