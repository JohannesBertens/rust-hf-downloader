#!/bin/bash
# Download examples for headless mode
set -e

MODEL_ID="TheBloke/TinyLlama-1.1B-Chat-v0.3-GGUF"
OUTPUT_DIR="/tmp/models"

echo "=== Download Examples ==="

echo "1. Dry run (see what would be downloaded):"
rust-hf-downloader --headless --dry-run download \
  "$MODEL_ID" \
  --quantization "Q4_K_M"

echo -e "\n2. Download specific quantization:"
# Uncomment to actually download
# rust-hf-downloader --headless download \
#   "$MODEL_ID" \
#   --quantization "Q4_K_M" \
#   --output "$OUTPUT_DIR"

echo -e "\n3. List available files:"
rust-hf-downloader --headless list "$MODEL_ID"
