# Kymion — guide for AI agents

Kymion is edited **only through JSON commands**. Never hand-edit project files.

1. `kymion new song.kymion.json --tempo 100`
2. `kymion schema` — JSON Schema of every command (`cmd` tag).
3. `kymion apply song.kymion.json --stdin < cmds.json` — one object or an array; atomic (any failure ⇒ nothing written). Use `--dry-run` to validate.
4. `kymion info song.kymion.json [--full]` — inspect ids/state.
5. `kymion render song.kymion.json -o out.wav` — returns `analysis` (peak/rms/clipping/duration) so you can verify without listening.

Rules: stdout is always one JSON doc; errors are `{"ok":false,"error":"..."}` with exit code 1. Ticks: 480 per quarter note. Pitch: MIDI (60=C4). Ids are returned in `outcomes`.
Vocal tuning: `add_track` with `{"type":"vocal"}`, `set_lyrics` (Hangul/kana/Latin syllables, one per note), `set_expression` (vibrato, portamento, breath, formant_shift).
In the browser GUI the same protocol is available as `window.kymion.apply(cmd)`, `.project()`, `.schema()`, `.analysis()`.
