# Kymion architecture (v0.1 draft)

```
            ┌────────────┐   ┌────────────┐
 AI agent ─▶│ kymion CLI │   │ Web GUI    │◀─ human
            └─────┬──────┘   │ (WASM+JS)  │
                  │          └─────┬──────┘
                  ▼                ▼
             kymion-core  (Project, Command, Engine + undo/redo, JSON Schema)
                  │
         kymion-audio (renderer, mixer, analysis, WAV)
            │                 │
      kymion-vocal        kymion-dsp
  (lyric→phoneme, pitch   (osc, ADSR, biquad, noise)
   curves, Voice trait)
```

## Principles
- **One mutation path**: every edit is a serialisable `Command`; GUI, CLI and WASM share it. Undo/redo is snapshot-based (simple, correct; switch to inverse-ops when projects get large).
- **Deterministic, offline rendering** so agents can render + `analyze` and iterate without audio hardware.
- **Pure Rust, no native deps in core** → identical code compiles to `wasm32-unknown-unknown`.
- **Vocal backend seam**: `kymion_vocal::Voice` trait. v0.1 ships a rule-based formant synthesizer; voicebank (concatenative, UTAU-style) and neural backends plug in here.

## Roadmap
1. MIDI import/export, audio clip tracks, tempo/time-signature maps.
2. Effects chain (EQ, comp, reverb, delay) + automation lanes; AudioWorklet realtime playback.
3. Vocal: pitch-curve editing (hand-drawn bends), phoneme timing editor, voicebank loader, neural vocoder backend.
4. Agent tooling: MCP server wrapping the CLI; `diff`/`explain` commands; score→text summaries.
