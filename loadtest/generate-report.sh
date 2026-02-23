#!/usr/bin/env bash
# generate-report.sh — Combine individual k6 JSON results into a
# markdown comparison table with Mermaid charts and a single JSON summary.
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

# ── Build sorted data index ──────────────────────────────────────────────
# Collect all unique rates (sorted numerically) and topologies (sorted alpha).
RATES=()
TOPOLOGIES=()
for f in "${FILES[@]}"; do
  RATES+=($(jq -r '.target_rate' "$f"))
  TOPOLOGIES+=($(jq -r '.topology' "$f"))
done
# Deduplicate and sort
RATES=($(printf '%s\n' "${RATES[@]}" | sort -un))
TOPOLOGIES=($(printf '%s\n' "${TOPOLOGIES[@]}" | sort -u))

# Lookup helper: find file matching topology + rate
find_file() {
  local topo="$1" rate="$2"
  for f in "${FILES[@]}"; do
    local ft=$(jq -r '.topology' "$f")
    local fr=$(jq -r '.target_rate' "$f")
    if [ "$ft" = "$topo" ] && [ "$fr" = "$rate" ]; then
      echo "$f"
      return
    fi
  done
}

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

# ── Mermaid charts ───────────────────────────────────────────────────────
# Only generate charts if we have more than one data point.
if [ ${#FILES[@]} -gt 1 ]; then

  # Build x-axis labels from rates
  x_labels=""
  for r in "${RATES[@]}"; do
    [ -n "$x_labels" ] && x_labels="${x_labels}, "
    if [ "$r" -ge 1000 ]; then
      x_labels="${x_labels}\"$(( r / 1000 ))k/s\""
    else
      x_labels="${x_labels}\"${r}/s\""
    fi
  done

  # ── Chart 1: Throughput ──────────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## Throughput" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo '```mermaid' >> "$OUTPUT_MD"
  echo "xychart-beta" >> "$OUTPUT_MD"
  echo "    title \"Actual Throughput (iterations/sec)\"" >> "$OUTPUT_MD"
  echo "    x-axis \"Target Rate\" [${x_labels}]" >> "$OUTPUT_MD"
  echo "    y-axis \"Iterations/sec\"" >> "$OUTPUT_MD"

  for topo in "${TOPOLOGIES[@]}"; do
    vals=""
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      if [ -n "$f" ]; then
        v=$(jq -r '.metrics.iterations_per_sec // 0 | round' "$f")
      else
        v=0
      fi
      [ -n "$vals" ] && vals="${vals}, "
      vals="${vals}${v}"
    done
    echo "    bar [${vals}]" >> "$OUTPUT_MD"
  done

  echo '```' >> "$OUTPUT_MD"
  # Legend for bar series (mermaid doesn't label multiple bar series)
  echo "" >> "$OUTPUT_MD"
  legend=""
  i=1
  for topo in "${TOPOLOGIES[@]}"; do
    [ -n "$legend" ] && legend="${legend} · "
    legend="${legend}**Series ${i}**: ${topo}"
    i=$((i + 1))
  done
  echo "> ${legend}" >> "$OUTPUT_MD"

  # ── Chart 2: P95 Latency ────────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## P95 Latency" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo '```mermaid' >> "$OUTPUT_MD"
  echo "xychart-beta" >> "$OUTPUT_MD"
  echo "    title \"P95 Execution Latency (ms)\"" >> "$OUTPUT_MD"
  echo "    x-axis \"Target Rate\" [${x_labels}]" >> "$OUTPUT_MD"
  echo "    y-axis \"Latency (ms)\"" >> "$OUTPUT_MD"

  for topo in "${TOPOLOGIES[@]}"; do
    vals=""
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      if [ -n "$f" ]; then
        v=$(jq -r '.metrics.js_exec_duration_p95 // 0 | . * 100 | round / 100' "$f")
      else
        v=0
      fi
      [ -n "$vals" ] && vals="${vals}, "
      vals="${vals}${v}"
    done
    echo "    bar [${vals}]" >> "$OUTPUT_MD"
  done

  echo '```' >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo "> ${legend}" >> "$OUTPUT_MD"

  # ── Chart 3: Success Rate ───────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## Success Rate" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo '```mermaid' >> "$OUTPUT_MD"
  echo "xychart-beta" >> "$OUTPUT_MD"
  echo "    title \"Success Rate (%)\"" >> "$OUTPUT_MD"
  echo "    x-axis \"Target Rate\" [${x_labels}]" >> "$OUTPUT_MD"
  echo "    y-axis \"Success %\" 0 --> 100" >> "$OUTPUT_MD"

  for topo in "${TOPOLOGIES[@]}"; do
    vals=""
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      if [ -n "$f" ]; then
        v=$(jq -r '.metrics.js_exec_success_rate // 0 | . * 1000 | round / 10' "$f")
      else
        v=0
      fi
      [ -n "$vals" ] && vals="${vals}, "
      vals="${vals}${v}"
    done
    echo "    bar [${vals}]" >> "$OUTPUT_MD"
  done

  echo '```' >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo "> ${legend}" >> "$OUTPUT_MD"

fi

# ── Notes ────────────────────────────────────────────────────────────────
cat >> "$OUTPUT_MD" << 'FOOTER'

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1 per iteration)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft
FOOTER

echo "Report written to ${OUTPUT_MD}"
echo "Summary written to ${OUTPUT_JSON}"
cat "$OUTPUT_MD"
