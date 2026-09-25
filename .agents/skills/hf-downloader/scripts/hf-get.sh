#!/usr/bin/env bash
# hf-get.sh — probe→select→download wrapper for rust-hf-downloader.
#
# Usage:
#   hf-get.sh <author/model> [--quant TYPE | --file PATH … | --all] [extra flags…]
#
# Without a selector, the script probes with no selector on purpose (exit 64
# + error.available[]), picks a file by heuristic, and re-invokes once:
#   - single .gguf files preferred over multipart parts (a lone
#     "-000XX-of-000YY.gguf" part is not a complete model)
#   - pick policy: largest (default) | smallest | first — override with
#     HF_GET_PICK=smallest|largest|first. Note: this picks a FILE from the
#     available list, not necessarily "the model" (e.g. smallest may pick a
#     112-byte tokenizer file); use --quant/--file/--all for real control
# Exit code is propagated from the binary; NDJSON events stream to stdout.
# Requires: jq, rust-hf-downloader >= 2.3.0.
set -euo pipefail

usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 64; }
[ $# -ge 1 ] && [ "$1" != "-h" ] && [ "$1" != "--help" ] || usage

MODEL=$1; shift
BIN=${HF_DOWNLOADER_BIN:-rust-hf-downloader}

SELECTOR=()
EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --quant|--file) SELECTOR+=("$1" "$2"); shift 2 ;;
    --all)          SELECTOR+=("$1"); shift ;;
    *)              EXTRA+=("$1"); shift ;;
  esac
done

run() { "$BIN" download "$MODEL" "${SELECTOR[@]+"${SELECTOR[@]}"}" "${EXTRA[@]+"${EXTRA[@]}"}" --json; }

if [ ${#SELECTOR[@]} -gt 0 ]; then
  set +e; run; code=$?; set -e
  exit "$code"
fi

# --- Probe: no selector, expect exit 64 + error.available -------------------
set +e
out=$(run 2>/dev/null)
code=$?
set -e

if [ "$code" -eq 0 ]; then
  printf '%s\n' "$out"   # single-file repo: already done, replay events
  exit 0
fi

err=$(printf '%s\n' "$out" | tail -1)   # error event is always the last line
if [ "$code" -ne 64 ] || { case $(echo "$err" | jq -r '.code') in
  ambiguous|no_files_match) false ;; *) true ;; esac; }; then
  echo "$err"
  exit "$code"
fi

# --- Pick from available -----------------------------------------------------
pick=${HF_GET_PICK:-largest}
# Multipart parts (…-00001-of-00002.gguf) are not standalone models.
complete=$(echo "$err" | jq -r '.available[].filename' | grep -v -- '-[0-9]*-of-[0-9]*\.gguf$' || true)
if [ -z "$complete" ]; then
  echo "error: only multipart GGUF parts available; re-invoke with --quant <TYPE>" >&2
  echo "$err"
  exit 64
fi

case "$pick" in
  largest)  order='-r' ;;
  smallest|first) order='' ;;
  *) echo "HF_GET_PICK must be largest|smallest|first" >&2; exit 64 ;;
esac

if [ "$pick" = first ]; then
  file=$(echo "$complete" | head -1)
else
  file=$(echo "$err" | jq -r '.available[] | "\(.size_bytes)\t\(.filename)"' \
    | grep -Ff <(echo "$complete") | sort -n $order -k1,1 | awk -F'\t' '{print $2}' | head -1)
fi

echo "probe: picking $file (HF_GET_PICK=$pick)" >&2
SELECTOR=(--file "$file")

set +e; run; code=$?; set -e
exit "$code"
