#!/bin/bash
# Search examples for headless mode
set -e

echo "=== Search Examples ==="

echo "1. Basic search:"
rust-hf-downloader --headless search "llama"

echo -e "\n2. Popular models:"
rust-hf-downloader --headless search "gpt" \
  --min-downloads 10000 \
  --min-likes 100

echo -e "\n3. JSON output:"
rust-hf-downloader --headless --json search "stable diffusion" | \
  jq '.results[] | {id: .id, downloads: .downloads}'

echo -e "\n4. Recently updated:"
rust-hf-downloader --headless search "llama" --sort modified
