#!/usr/bin/env bash
# Build the WASM engine into web/pkg, then serve web/ at http://localhost:8080
set -euo pipefail
cd "$(dirname "$0")/.."
wasm-pack build crates/kymion-wasm --release --target web --out-dir ../../web/pkg
[[ "${1:-}" == "--serve" ]] && python3 -m http.server 8080 -d web
