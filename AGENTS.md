# Kymion — guide for AI agents

Kymion is edited **only through JSON commands**. Never hand-edit project files.

## Workflow
1. `kymion new song.kymion.json --tempo 120`
2. `kymion schema` — JSON Schema of every command (`cmd` tag).
3. `kymion apply song.kymion.json --stdin < cmds.json` — one object or an array; atomic (any failure ⇒ nothing written). Use `--dry-run` to validate.
4. `kymion info song.kymion.json [--full]` — inspect ids/state.
5. `kymion render song.kymion.json -o out.wav` — returns `analysis` (peak/rms/clipping/duration) so you can verify without listening.

## Standard MIDI File (SMF) Interop
- `kymion import-midi song.mid -o song.kymion.json` — imports tracks, notes, tempo, and lyric events.
- `kymion export-midi song.kymion.json -o song.mid` — exports standard format 1 MIDI file.

## Protocol Rules
- Standard output is always one JSON document; errors are `{"ok":false,"error":"..."}` with exit code 1.
- Ticks: 480 PPQ (ticks per quarter note). Pitch: MIDI note number (60 = C4).
- Ids are returned in `outcomes` (e.g. `track_id`, `note_id`, `fx_id`).

## Vocal Tuning & Expressions
- `add_track` with `{"type":"vocal"}`
- `set_lyrics` with `["Hangul", "kana", "Latin syllables"]` (one per note)
- `set_expression`: vibrato (depth, rate, delay), portamento glide, breath, formant_shift.

## Audio Effects (Inserts & Master Bus)
- Add Track FX: `{"cmd":"add_fx","track":1,"kind":{"type":"reverb","room_size":0.7,"damping":0.3,"width":1.0,"mix":0.35}}`
- Add Master FX: `{"cmd":"add_fx","track":null,"kind":{"type":"compressor","threshold_db":-12.0,"ratio":4.0,"attack_ms":10.0,"release_ms":100.0,"makeup_db":2.0}}`
- Types: `eq` (3-band parametric), `delay` (stereo ping-pong), `reverb` (algorithmic), `compressor`.
- Control: `set_fx` (toggle bypass, adjust parameters), `remove_fx`.

In the browser GUI the same protocol is available as `window.kymion.apply(cmd)`, `.project()`, `.schema()`, `.analysis()`, `.importMidi(bytes)`, `.exportMidi()`.
