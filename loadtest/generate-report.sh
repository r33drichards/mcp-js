#!/usr/bin/env bash
# generate-report.sh — Combine individual k6 JSON results into a
# markdown comparison table with text-based charts and a single JSON summary.
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

# ── Text-based visual charts ─────────────────────────────────────────────
# Unicode bar charts that render in any markdown viewer (GitHub, terminals, etc.)
if [ ${#FILES[@]} -gt 1 ]; then

  # Helper: draw a bar of width proportional to value/max (max 30 chars)
  draw_bar() {
    local val="$1" max_val="$2" max_width=30
    if [ "$max_val" -eq 0 ] || [ "$val" -eq 0 ]; then
      echo ""
      return
    fi
    local width=$(( val * max_width / max_val ))
    [ "$width" -lt 1 ] && width=1
    printf '%0.s█' $(seq 1 "$width")
  }

  # Log-scale bar: width proportional to log(value)/log(max)
  draw_bar_log() {
    local val="$1" max_val="$2" max_width=30
    if [ "$max_val" -le 1 ] || [ "$val" -le 0 ]; then
      [ "$val" -gt 0 ] && echo "█" || echo ""
      return
    fi
    # Use awk for floating-point log math
    local width
    width=$(awk -v v="$val" -v m="$max_val" -v w="$max_width" \
      'BEGIN { if(v<=0||m<=1){print 1}else{r=log(v)/log(m)*w; if(r<1)r=1; printf "%d",r+0.5} }')
    printf '%0.s█' $(seq 1 "$width")
  }

  # ── Chart 1: Throughput ──────────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## Throughput" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo "| Topology | Rate | Actual | |" >> "$OUTPUT_MD"
  echo "|----------|------|--------|-|" >> "$OUTPUT_MD"

  # Find max throughput for scaling bars
  max_iters=0
  for f in "${FILES[@]}"; do
    v=$(jq -r '.metrics.iterations_per_sec // 0 | round' "$f")
    [ "$v" -gt "$max_iters" ] && max_iters="$v"
  done

  for topo in "${TOPOLOGIES[@]}"; do
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      [ -z "$f" ] && continue
      v=$(jq -r '.metrics.iterations_per_sec // 0 | round' "$f")
      bar=$(draw_bar "$v" "$max_iters")
      echo "| ${topo} | ${r}/s | ${v}/s | \`${bar}\` |" >> "$OUTPUT_MD"
    done
  done

  # ── Chart 2: P95 Latency ────────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## P95 Latency" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo "| Topology | Rate | P95 (ms) | |" >> "$OUTPUT_MD"
  echo "|----------|------|----------|-|" >> "$OUTPUT_MD"

  max_p95=0
  for f in "${FILES[@]}"; do
    v=$(jq -r '.metrics.js_exec_duration_p95 // 0 | round' "$f")
    [ "$v" -gt "$max_p95" ] && max_p95="$v"
  done
  [ "$max_p95" -eq 0 ] && max_p95=1

  for topo in "${TOPOLOGIES[@]}"; do
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      [ -z "$f" ] && continue
      v_raw=$(jq -r '.metrics.js_exec_duration_p95 // 0 | . * 100 | round / 100' "$f")
      v_int=$(jq -r '.metrics.js_exec_duration_p95 // 0 | round' "$f")
      bar=$(draw_bar_log "$v_int" "$max_p95")
      echo "| ${topo} | ${r}/s | ${v_raw} | \`${bar}\` |" >> "$OUTPUT_MD"
    done
  done

  # ── Chart 3: Success Rate ───────────────────────────────────────────
  echo "" >> "$OUTPUT_MD"
  echo "## Success Rate" >> "$OUTPUT_MD"
  echo "" >> "$OUTPUT_MD"
  echo "| Topology | Rate | Success | |" >> "$OUTPUT_MD"
  echo "|----------|------|--------|-|" >> "$OUTPUT_MD"

  for topo in "${TOPOLOGIES[@]}"; do
    for r in "${RATES[@]}"; do
      f=$(find_file "$topo" "$r")
      [ -z "$f" ] && continue
      v=$(jq -r '.metrics.js_exec_success_rate // 0 | . * 1000 | round / 10' "$f")
      v_int=$(jq -r '.metrics.js_exec_success_rate // 0 | . * 100 | round' "$f")
      bar=$(draw_bar "$v_int" 100)
      echo "| ${topo} | ${r}/s | ${v}% | \`${bar}\` |" >> "$OUTPUT_MD"
    done
  done

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
