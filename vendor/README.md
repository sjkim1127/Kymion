# vendor/ — reference only

Shallow snapshots of open-source DAWs / singing-voice tools, kept **only for reading and learning architecture**.

**Rules**
- Never `use`, link, depend on, copy, or build any of this code. It is **not** a library for Kymion.
- Do not copy code or assets into `crates/` — most of these are GPL/LGPL/MIT with different terms than Kymion (Apache-2.0).
- `vendor/` is git-ignored (only this README is tracked) and excluded from the Cargo workspace.

| dir | project | license | look at it for |
|---|---|---|---|
| `OpenUtau` | https://github.com/stakira/OpenUtau | MIT | UTAU-style vocal editing, phonemizers, pitch curves, renderers |
| `DiffSinger` | https://github.com/openvpi/DiffSinger | Apache-2.0 | neural singing-voice synthesis pipeline |
| `ardour` | https://github.com/Ardour/ardour | GPL-2.0+ | pro DAW engine: session, routing, automation |
| `lmms` | https://github.com/LMMS/lmms | GPL-2.0+ | pattern/piano-roll workflow, instrument/FX plugins |
| `zrythm` | https://github.com/zrythm/zrythm | AGPL-3.0 | modern DAW architecture, timeline/arranger |
| `audacity` | https://github.com/audacity/audacity | GPL-2.0+ | audio editing, project model, effects |
| `tracktion_engine` | https://github.com/tracktion/tracktion_engine | GPL/commercial | DAW engine design (edit/track/clip model) |
| `helio-workstation` | https://github.com/helio-fm/helio-workstation | GPL-3.0 | sequencer UX, MIDI editing |
| `Meadowlark` | https://github.com/MeadowlarkDAW/Meadowlark | GPL-3.0 | Rust DAW design (closest to Kymion's stack) |

Snapshots are `--depth 1`; `.git` was removed. Re-fetch with:
`git clone --depth 1 <url> vendor/<dir>`
