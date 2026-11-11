<p align="center"><img src="assets/logo.svg" width="340" alt="Kymion"></p>

# Kymion

Professional music editing DAW + audio engine + vocaloid-style vocal tuning, written in Rust.
**Web GUI runs in the browser via WASM** (no desktop GUI required), and an **agent-first JSON CLI** drives the exact same engine.

> Status: v0.3 basic DAW features. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [AGENTS.md](AGENTS.md).

## Features
- **DAW Core**: Multi-track timeline (MIDI, vocal and audio tracks), 480 PPQ precision, mixer with gain/pan/mute/solo, volume & pan automation, time signatures, loop region, metronome, non-destructive WAV audio clips.
- **Real-Time AudioWorklet Engine**: Zero-latency streaming playback with sub-millisecond hardware-accurate playhead tracking and live parameter tweaking during playback.
- **Audio Clip Editing**: Start offset trimming, duration trimming, equal-power fade-in and fade-out ramps, clip gain adjustment, and playhead clip splitting (`split_clip`).
- **Mixer Meters & Stereo Pan**: Real-time peak & RMS meters for individual tracks and master bus output with visual dB indicators and responsive pan controls.
- **Web DAW GUI**: arrangement view + piano roll, bar/beat ruler, playhead & loop playback, draw/select tools, multi-select move/resize, copy/paste/duplicate/split, velocity lane, zoom/scroll, undo/redo shortcuts.
- **Standard MIDI Interop**: Lossless `.mid` import & export with tempo map and lyric synchronization.
- **DSP Effect Chains**: Track insert FX and master bus chain with 3-band parametric EQ, Stereo Ping-Pong Delay, Algorithmic Reverb, and Dynamic Range Compressor.
- **Vocaloid-Style Vocal Tuning**: Lyric-to-phoneme parsing (Hangul / Kana / Latin), formant singing synthesis, vibrato delay/depth/rate, portamento glide, breathiness, and gender formant shift.
- **Agent-First Command Architecture**: 100% of mutations run through atomic, validated JSON commands with full undo/redo and JSON Schema introspection.
- **Zero-Install WASM GUI**: Piano roll, interactive FX rack, vocal tuner, and WebAudio playback running purely client-side.

## Quick start
```bash
# Build the CLI
cargo build --release -p kymion-cli
K=target/release/kymion

# Create project, import/export MIDI
$K new song.kymion.json --tempo 128
$K import-midi my_track.mid -o song.kymion.json
$K export-midi song.kymion.json -o exported.mid

# Apply commands (tracks, notes, lyrics, effects)
$K apply song.kymion.json --stdin < examples/demo.commands.json

# Offline audio render with peak/RMS/clipping analysis
$K render song.kymion.json -o song.wav

# Run the WASM Web GUI
./scripts/build-web.sh --serve              # Opens at http://localhost:8080
```

## Crates
| crate | role |
|---|---|
| `kymion-core` | project model, command protocol, undo/redo, Standard MIDI File (SMF) import/export, JSON Schema |
| `kymion-dsp` | oscillators, ADSR, 3-band parametric EQ, stereo delay, algorithmic reverb, compressor |
| `kymion-vocal` | lyric→phoneme (한글/かな/Latin), formant singing synth, vibrato/portamento, Voice trait |
| `kymion-audio` | multi-track mixer, track & master FX chain renderer, audio analysis, 16-bit PCM WAV export |
| `kymion-cli` | `kymion` binary (JSON in/out, MIDI tools, render engine) |
| `kymion-wasm` | wasm-bindgen engine driving the Web DAW at `web/` |

License: Apache-2.0
