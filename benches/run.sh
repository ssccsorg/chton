#!/bin/bash
set -e
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
COMMIT_HASH=$(cd "$(dirname "$0")/.." && git rev-parse --short HEAD 2>/dev/null || echo "unknown")

cd "$(dirname "$0")"
SCRIPT_DIR="$(pwd)"
RESULT_DIR="$SCRIPT_DIR/result"
mkdir -p "$RESULT_DIR"

echo "=== Chton Benchmark Suite ==="
echo "Timestamp: $TIMESTAMP"
echo "Commit:    $COMMIT_HASH"
echo ""

echo "=== Running origin benchmarks ==="
cargo bench -p chton-bench -- origin 2>&1 | tee "$RESULT_DIR/output-${TIMESTAMP}.txt" || true

echo "=== Running binding benchmarks ==="
cargo bench -p chton-bench -- binding 2>&1 | tee -a "$RESULT_DIR/output-${TIMESTAMP}.txt" || true

echo "=== Running map benchmarks (reopen + spatial included) ==="
cargo bench -p chton-bench -- map 2>&1 | tee -a "$RESULT_DIR/output-${TIMESTAMP}.txt" || true

echo "=== Running spatial benchmarks ==="
cargo bench -p chton-bench -- spatial 2>&1 | tee -a "$RESULT_DIR/output-${TIMESTAMP}.txt" || true

echo "=== Running store benchmarks ==="
cargo bench -p chton-bench -- store 2>&1 | tee -a "$RESULT_DIR/output-${TIMESTAMP}.txt" || true

echo "=== Exporting results ==="
python3 "$SCRIPT_DIR/export_results.py" "$RESULT_DIR/bench-${TIMESTAMP}-${COMMIT_HASH}.json"

echo ""
echo "=== Done ==="
echo "Raw output:  $RESULT_DIR/output-${TIMESTAMP}.txt"
echo "JSON result: $RESULT_DIR/bench-${TIMESTAMP}-${COMMIT_HASH}.json"
