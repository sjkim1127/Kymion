# Kymion architecture (v0.2)

```
            ┌──────────────────┐   ┌──────────────────────────┐
 AI agent ─▶│    kymion CLI    │   │         Web GUI          │◀─ human / producer
            │ (JSON in/out/mid)│   │(WASM + PianoRoll + FX UI)│
            └────────┬─────────┘   └────────────┬─────────────┘
                     │                          │
                     ▼                          ▼
             kymion-core (Project, Command, Engine + undo/redo, SMF MIDI I/O)
                     │
             kymion-audio (offline renderer, multi-track mixer, FX chain)
               │          │
         kymion-vocal     kymion-dsp (Osc, ADSR, 3-band EQ, Delay, Reverb, Comp)
   (lyric→phoneme, pitch
    curves, Voice trait)
```

## Core Principles
- **Logic / Cubase DAW Model**: Multi-track composition, standard MIDI file interop, track insert effects, and master bus summing.
- **Single mutation path**: Every edit (tracks, notes, lyrics, mix, FX) is a serialisable `Command`. GUI, CLI and AI agents share 100% identical state mutations.
- **Deterministic, offline rendering**: AI agents can generate arrangements, tweak EQ/Reverb, and run `analyze` to verify peak, RMS, and clipping metrics without needing an audio output device.
- **Pure Rust, no native C dependencies**: Compiles cleanly to both native CLI and `wasm32-unknown-unknown` for zero-install in-browser production.
- **Pluggable Vocal Engine**: `kymion_vocal::Voice` trait allows swapping the current rule-based formant synth with sample-based voicebanks or neural vocoders.

## Signal Flow
1. **Track Synth / Vocal**:
   - `Instrument` -> PolyBLEP Osc -> ADSR envelope.
   - `Vocal` -> Lyric-to-phoneme -> Glottal pulse + Noise -> Formant biquad resonators -> Vibrato/Portamento.
2. **Track FX Chain**:
   - 3-band Parametric EQ -> Stereo Ping-Pong Delay -> Algorithmic Reverb -> Compressor.
3. **Mixer Bus**:
   - Equal-power panning -> Track gain -> Master summing.
4. **Master Bus**:
   - Master EQ -> Master Compressor / Bus Processor -> Soft Limiter (`tanh`).
