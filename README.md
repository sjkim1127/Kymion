<p align="center"><img src="assets/logo.svg" width="320" alt="Kymion"></p>

# Kymion

Music editor + audio engine + vocal (vocaloid-style) tuning, written in Rust.
**GUI runs in the browser via WASM** (no desktop GUI), and an **agent-friendly JSON CLI** drives the exact same engine.

> Status: v0.1 initial design/skeleton. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [AGENTS.md](AGENTS.md).

## Quick start
```bash
cargo build --release -p kymion-cli
K=target/release/kymion
$K new song.kymion.json --tempo 100
$K apply song.kymion.json --stdin < examples/demo.commands.json
$K render song.kymion.json -o song.wav      # prints peak/rms/clipping analysis

./scripts/build-web.sh --serve              # GUI at http://localhost:8080
```

## Crates
| crate | role |
|---|---|
| `kymion-core` | project model, command protocol, undo/redo, JSON Schema |
| `kymion-dsp` | oscillators, ADSR, filters |
| `kymion-vocal` | lyric→phoneme (한글/かな/Latin), formant singing synth, vibrato/portamento |
| `kymion-audio` | mixer, renderer, analysis, WAV |
| `kymion-cli` | `kymion` binary (JSON in/out) |
| `kymion-wasm` | wasm-bindgen engine used by `web/` |

License: Apache-2.0
