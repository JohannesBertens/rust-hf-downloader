#!/bin/bash
# Timing test for headless mode download
# Tests: unsloth/Qwen3-4B-Instruct-2507-GGUF at IQ1_M quantization

set -e

MODEL_ID="unsloth/Qwen3-4B-Instruct-2507-GGUF"
QUANTIZATION="IQ1_M"
OUTPUT_DIR="/tmp/rust-hf-downloader-timing-test"

echo "=== Rust HF Downloader Timing Test ==="
echo "Model: $MODEL_ID"
echo "Quantization: $QUANTIZATION"
echo "Output directory: $OUTPUT_DIR"
echo ""

# Delete existing output directory if it exists
if [ -d "$OUTPUT_DIR" ]; then
    echo "Removing existing download at: $OUTPUT_DIR"
    rm -rf "$OUTPUT_DIR"
fi

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Time the dry run to measure API response time
echo "1. Timing dry run (measuring API response time)..."
START_TIME=$(date +%s.%N)
cargo run --release -- --headless --dry-run download \
  "$MODEL_ID" \
  --quantization "$QUANTIZATION" \
  --output "$OUTPUT_DIR" > /tmp/dry-run-output.txt 2>&1
END_TIME=$(date +%s.%N)
DRY_RUN_TIME=$(echo "$END_TIME - $START_TIME" | bc)
echo "Dry run completed in: ${DRY_RUN_TIME}s"
echo ""

# Extract file count and total size from dry run output
FILE_COUNT=$(grep 'Files to download:' /tmp/dry-run-output.txt | grep -oE '[0-9]+')
TOTAL_SIZE=$(grep 'Total size:' /tmp/dry-run-output.txt | awk '{print $3, $4}')
echo "Files to download: $FILE_COUNT"
echo "Total size: $TOTAL_SIZE"
echo ""

# Convert total size to MB for calculation
TOTAL_SIZE_MB=$(grep 'Total size:' /tmp/dry-run-output.txt | awk '{
    size = $3;
    unit = $4;
    if (unit == "GB") printf "%.2f", size * 1024;
    else if (unit == "MB") printf "%.2f", size;
    else if (unit == "KB") printf "%.2f", size / 1024;
    else printf "%.2f", size / (1024 * 1024);
}')

# Time the actual download
echo "2. Timing actual download..."
START_TIME=$(date +%s.%N)
cargo run --release -- --headless download \
  "$MODEL_ID" \
  --quantization "$QUANTIZATION" \
  --output "$OUTPUT_DIR" > /tmp/download-output.txt 2>&1
END_TIME=$(date +%s.%N)
DOWNLOAD_TIME=$(echo "$END_TIME - $START_TIME" | bc)

# Calculate average download speed
DOWNLOAD_SPEED=$(echo "scale=2; $TOTAL_SIZE_MB / $DOWNLOAD_TIME" | bc)
echo "Download completed in: ${DOWNLOAD_TIME}s"
echo "Average speed: ${DOWNLOAD_SPEED} MB/s"
echo ""

# Calculate summary
echo "=== Timing Test Summary ==="
echo "Model: $MODEL_ID"
echo "Quantization: $QUANTIZATION"
echo "Files: $FILE_COUNT"
echo "Total size: $TOTAL_SIZE ($TOTAL_SIZE_MB MB)"
echo "Dry run time: ${DRY_RUN_TIME}s"
echo "Download time: ${DOWNLOAD_TIME}s"
echo "Average speed: ${DOWNLOAD_SPEED} MB/s"
echo ""

# Cleanup temporary files
rm -f /tmp/dry-run-output.txt /tmp/download-output.txt

echo "Timing test completed!"
