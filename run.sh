#!/usr/bin/env bash
set -euo pipefail
#
# chton — Single entry point
#
# Usage:
#   ./run.sh                 # Full pipeline: fix → check → test
#   ./run.sh --check         # fmt → clippy → build → test (strict)
#   ./run.sh --fix           # auto-fix → build → test
#   ./run.sh --bench         # build + test + benchmarks (harness pending)
#   ./run.sh --doc           # build documentation
#   ./run.sh --help
#

cd "$(dirname "$0")"
export RUSTFLAGS="-D warnings"

# ── Helpers ───────────────────────────────────────────────────────────

check_checks() {
    echo "--- fmt (check) ---"
    cargo fmt --check
    echo "--- clippy (all targets) ---"
    cargo clippy --all-targets
    echo "--- build + test (release) ---"
    cargo build --release
    cargo test --release
    echo "--- no_std anchors (no-default-features) ---"
    cargo check --no-default-features
    cargo test --no-default-features --test no_std_anchors
    echo "--- wasm32-unknown-unknown check (true no_std target) ---"
    cargo check --no-default-features --target wasm32-unknown-unknown
    echo "--- riscv32imac-unknown-none-elf check (MCU target) ---"
    cargo check --no-default-features --target riscv32imac-unknown-none-elf
}

build_and_test() {
    echo "--- build + test (release) ---"
    cargo build --release
    cargo test --release
}

auto_fix() {
    echo "--- auto-fix ---"
    cargo fmt --all
    cargo clippy --fix --allow-dirty 2>&1 || true
    cargo fix --allow-dirty 2>&1 || true
    cargo fmt --all
}

build_docs() {
    echo "--- documentation ---"
    cargo doc --no-deps
}

# ── Dispatch ──────────────────────────────────────────────────────────

case "${1:-}" in
    --check|check)
        check_checks
        ;;
    --fix|fix)
        auto_fix
        build_and_test
        ;;
    --bench|bench)
        build_and_test
        echo "--- running benchmarks ---"
        (cargo bench 2>&1 | tail -20) || echo "benchmark harness not yet defined"
        ;;
    --doc|doc)
        build_docs
        ;;
    --help|-h)
        echo "Usage: ./run.sh [--check|--fix|--bench|--doc|--help]"
        exit 0
        ;;
    *)
        auto_fix
        check_checks
        ;;
esac
