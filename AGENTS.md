# Kymion — guide for AI agents

Kymion is edited **only through JSON commands**. Never hand-edit project files.

## Workflow
1. `kymion new song.kymion.json --tempo 120`
2. `kymion schema` — JSON Schema of every command (`cmd` tag).
3. `kymion apply song.kymion.json --stdin < cmds.json` — one object or an array; atomic (any failure ⇒ nothing written). Use `--dry-run` to validate.
4. `kymion info song.kymion.json [--full]` — inspect ids/state.
5. `kymion render song.kymion.json -o out.wav` — returns `analysis` (peak/rms/clipping/duration) so you can verify without listening.

## Standard MIDI File (SMF) & SoundFont (SF2) Interop
- `kymion import-midi song.mid -o song.kymion.json` — imports tracks, notes, tempo, and lyric events.
- `kymion export-midi song.kymion.json -o song.mid` — exports standard format 1 MIDI file.
- `kymion import-soundfont song.kymion.json instrument.sf2 [--track ID] [--preset P] [--bank B]` — imports SoundFont 2 multi-sample instrument and assigns it to a track.

## Protocol Rules
- Standard output is always one JSON document; errors are `{"ok":false,"error":"..."}` with exit code 1.
- Ticks: 480 PPQ (ticks per quarter note). Pitch: MIDI note number (60 = C4).
- Ids are returned in `outcomes` (e.g. `track_id`, `note_id`, `fx_id`, `asset_id`).

## DAW Operations
- Tracks: `instrument`, `vocal`, `audio`. `set_mix` has `gain/pan/mute/solo`; `rename_track`.
- Virtual Instruments & Subtractive Synthesizer: `set_track_synth {track, synth: {filter: moog|lowpass|highpass|bandpass|none, cutoff_hz, resonance, filter_env_amount, attack, decay, sustain, release}}`, `set_track_waveform {track, waveform: pluck|saw|square|sine|triangle}`, `set_track_soundfont {track, soundfont: {asset, bank?, preset?}}` (set `soundfont: null` to return to synth oscillator).
- AI Diagnostics: `kymion diagnose song.kymion.json` performs ITU-R BS.1770-4 gated loudness measurement (LUFS, True Peak, LRA) and 6-band frequency masking clash detection with mixing suggestions. `kymion analyze` returns peak, RMS, clipping, duration, and LUFS.
- Transport/grid: `set_time_signature {numerator, denominator}`, `set_loop {start,end}` (omit both to clear).
- Batch note edits (one undo step): `add_notes`, `remove_notes`, `move_notes {delta_tick, delta_pitch}`, `set_velocity`, `split_note`, `quantize`, `transpose`.
- Audio: `kymion import-audio song.kymion.json clip.wav [--track ID] [--start TICK]` embeds the WAV (mono) and places a clip; then `add_clip`, `move_clip`, `remove_clip`, `trim_clip {track, clip, start?, offset_ticks?, duration_ticks?}`, `split_clip {track, clip, at}`, `set_clip_gain {track, clip, gain}`, `set_clip_fade {track, clip, fade_in_ticks?, fade_out_ticks?}`.
- Automation: `add_auto_point {track, param: gain|pan, tick, value}`, `clear_auto`.
- `kymion render ... --metronome` mixes a click. Track/clip/note/asset ids share one counter.

## Vocal Tuning & Expressions
- `add_track` with `{"type":"vocal"}`
- `set_lyrics` with `["Hangul", "kana", "Latin syllables"]` (one per note)
- `set_expression`: vibrato (depth, rate, delay), portamento glide, breath, formant_shift.

## Audio Effects (Inserts & Master Bus)
- Add Track FX: `{"cmd":"add_fx","track":1,"kind":{"type":"reverb","room_size":0.7,"damping":0.3,"width":1.0,"mix":0.35}}`
- Add Master FX: `{"cmd":"add_fx","track":null,"kind":{"type":"compressor","threshold_db":-12.0,"ratio":4.0,"attack_ms":10.0,"release_ms":100.0,"makeup_db":2.0}}`
- Types: `eq` (3-band parametric), `delay` (stereo ping-pong), `reverb` (algorithmic), `compressor`.
- Control: `set_fx` (toggle bypass, adjust parameters), `remove_fx`.

In the browser GUI the same protocol is available as `window.kymion.apply(cmd)`, `.project()`, `.schema()`, `.analysis()`, `.importMidi(bytes)`, `.exportMidi()`, `.meters()`, `.previewNote(track, pitch, dur, vel)`.
