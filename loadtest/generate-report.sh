#!/usr/bin/env bash
# generate-report.sh — Combine individual k6 JSON results into a
# markdown comparison table and a single JSON summary.
set -euo pipefail

RESULTS_DIR="${1:-.}"
OUTPUT_MD="${RESULTS_DIR}/benchmark-report.md"
OUTPUT_JSON="${RESULTS_DIR}/benchmark-summary.json"

# Collect all result files
shopt -s nullglob
FILES=("${RESULTS_DIR}"/results-*.json)
shopt -u nullglob

if [ ${#FILES[@]} -eq 0 ]; then
  echo "No result files found in ${RESULTS_DIR}" >&2
  exit 1
fi

# ── JSON summary ─────────────────────────────────────────────────────────
echo "[" > "$OUTPUT_JSON"
first=true
for f in "${FILES[@]}"; do
  if [ "$first" = true ]; then first=false; else echo "," >> "$OUTPUT_JSON"; fi
  cat "$f" >> "$OUTPUT_JSON"
done
echo "]" >> "$OUTPUT_JSON"

# ── Markdown report ──────────────────────────────────────────────────────
cat > "$OUTPUT_MD" << 'HEADER'
# MCP-V8 Load Test Benchmark Report

Comparison of single-node vs 3-node cluster at various request rates.

## Results

| Topology | Target Rate | Actual Iter/s | HTTP Req/s | Exec Avg (ms) | Exec p95 (ms) | Exec p99 (ms) | Success % | Dropped | Max VUs |
|----------|-------------|---------------|------------|----------------|----------------|----------------|-----------|---------|---------|
HEADER

for f in "${FILES[@]}"; do
  topology=$(jq -r '.topology' "$f")
  target=$(jq -r '.target_rate' "$f")
  iters=$(jq -r '.metrics.iterations_per_sec // 0 | . * 10 | round / 10' "$f")
  http_rps=$(jq -r '.metrics.http_reqs_per_sec // 0 | . * 10 | round / 10' "$f")
  avg=$(jq -r '.metrics.js_exec_duration_avg // 0 | . * 100 | round / 100' "$f")
  p95=$(jq -r '.metrics.js_exec_duration_p95 // 0 | . * 100 | round / 100' "$f")
  p99=$(jq -r '.metrics.js_exec_duration_p99 // 0 | . * 100 | round / 100' "$f")
  success=$(jq -r '.metrics.js_exec_success_rate // 0 | . * 1000 | round / 10' "$f")
  dropped=$(jq -r '.metrics.dropped_iterations // 0' "$f")
  vus=$(jq -r '.metrics.vus_max // 0' "$f")

  echo "| ${topology} | ${target}/s | ${iters} | ${http_rps} | ${avg} | ${p95} | ${p99} | ${success}% | ${dropped} | ${vus} |" >> "$OUTPUT_MD"
done

cat >> "$OUTPUT_MD" << 'FOOTER'

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1:1 with iterations)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft
FOOTER

echo "Report written to ${OUTPUT_MD}"
echo "Summary written to ${OUTPUT_JSON}"
cat "$OUTPUT_MD"
