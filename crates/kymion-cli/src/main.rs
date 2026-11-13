//! `kymion` — agent-friendly CLI. Every command prints exactly one JSON
//! document on stdout; failures print `{"ok":false,"error":...}` and exit 1.
//! Project files are plain JSON, edits are JSON commands (see `kymion schema`).

use clap::{Parser, Subcommand};
use kymion_audio::{analyze, decode_wav, render_with, to_wav_bytes, RenderOptions};
use kymion_core::{
    command_schema,
    midi::{export_midi, import_midi},
    Command, Engine, Project, TrackKind,
};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kymion",
    version,
    about = "Music editing + audio engine + vocal tuning (agent-first CLI)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project file.
    New {
        path: PathBuf,
        #[arg(long, default_value = "Untitled")]
        name: String,
        #[arg(long, default_value_t = 120.0)]
        tempo: f32,
    },
    /// Print project summary (tracks, notes, length, effects).
    Info {
        path: PathBuf,
        #[arg(long)]
        full: bool,
    },
    /// Apply JSON command(s): one object or an array. Use --cmd or --stdin.
    Apply {
        path: PathBuf,
        #[arg(long)]
        cmd: Option<String>,
        #[arg(long)]
        stdin: bool,
        /// Write result here instead of editing in place.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Validate only; do not write.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the JSON Schema of the command protocol.
    Schema,
    /// Render project to a WAV file.
    Render {
        path: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Mix in a metronome click.
        #[arg(long)]
        metronome: bool,
    },
    /// Render and report peak/RMS/clipping/duration (verify without listening).
    Analyze { path: PathBuf },
    /// Diagnose project acoustics, ITU-R BS.1770 LUFS loudness, and frequency masking clashes.
    Diagnose { path: PathBuf },
    /// Import a WAV file as an audio asset and place it as a clip.
    ImportAudio {
        project: PathBuf,
        wav: PathBuf,
        /// Existing audio track id (a new audio track is created if omitted).
        #[arg(long)]
        track: Option<u32>,
        /// Clip start position in ticks.
        #[arg(long, default_value_t = 0)]
        start: u32,
    },
    /// Import a Standard MIDI File (.mid) into a Kymion project.
    ImportMidi {
        midi_path: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Export a Kymion project to a Standard MIDI File (.mid).
    ExportMidi {
        project_path: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Import a SoundFont 2 (.sf2) file as a project soundfont asset.
    ImportSoundfont {
        project: PathBuf,
        sf2: PathBuf,
        /// Optional custom name for the soundfont.
        #[arg(long)]
        name: Option<String>,
        /// Assign this soundfont to an instrument track id.
        #[arg(long)]
        track: Option<u32>,
        /// SoundFont bank number (default 0).
        #[arg(long, default_value_t = 0)]
        bank: u16,
        /// SoundFont preset/patch number (default 0).
        #[arg(long, default_value_t = 0)]
        preset: u16,
    },
}

fn run(cli: Cli) -> Result<Value, String> {
    let load = |p: &PathBuf| -> Result<Project, String> {
        Project::from_json(
            &std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
        )
        .map_err(|e| e.to_string())
    };
    match cli.cmd {
        Cmd::New { path, name, tempo } => {
            kymion_core::validate_tempo(tempo).map_err(|e| e.to_string())?;
            let mut p = Project::new(name);
            p.tempo = tempo;
            std::fs::write(&path, p.to_json()).map_err(|e| e.to_string())?;
            Ok(json!({"ok": true, "path": path}))
        }
        Cmd::Info { path, full } => {
            let p = load(&path)?;
            if full {
                return serde_json::to_value(&p).map_err(|e| e.to_string());
            }
            let tracks: Vec<Value> = p
                .tracks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "kind": t.kind,
                        "notes": t.notes.len(),
                        "fx": t.fx.len(),
                        "clips": t.clips.len(),
                        "mute": t.mute,
                        "solo": t.solo
                    })
                })
                .collect();
            Ok(json!({
                "ok": true,
                "name": p.name,
                "tempo": p.tempo,
                "time_signature": [p.time_sig_num, p.time_sig_den],
                "loop": p.loop_region,
                "assets": p.assets.iter().map(|a| json!({"id": a.id, "name": a.name, "duration_sec": a.duration_secs()})).collect::<Vec<_>>(),
                "length_ticks": p.length_ticks(),
                "length_sec": p.ticks_to_secs(p.length_ticks()),
                "tracks": tracks,
                "master_fx": p.master_fx.len()
            }))
        }
        Cmd::Apply {
            path,
            cmd,
            stdin,
            out,
            dry_run,
        } => {
            let raw = match (cmd, stdin) {
                (Some(c), _) => c,
                (None, true) => {
                    let mut s = String::new();
                    std::io::stdin()
                        .read_to_string(&mut s)
                        .map_err(|e| e.to_string())?;
                    s
                }
                _ => return Err("provide --cmd '<json>' or --stdin".into()),
            };
            let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            let cmds: Vec<Command> = match v {
                Value::Array(a) => a
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?,
                o => vec![serde_json::from_value(o).map_err(|e| e.to_string())?],
            };
            let mut e = Engine::new(load(&path)?);
            let mut outcomes = vec![];
            for (i, c) in cmds.into_iter().enumerate() {
                outcomes.push(
                    e.apply(c)
                        .map_err(|er| format!("command #{i} failed (nothing written): {er}"))?,
                );
            }
            if !dry_run {
                std::fs::write(out.as_ref().unwrap_or(&path), e.project.to_json())
                    .map_err(|e| e.to_string())?;
            }
            Ok(
                json!({"ok": true, "applied": outcomes.len(), "outcomes": outcomes, "dry_run": dry_run}),
            )
        }
        Cmd::Schema => serde_json::from_str(&command_schema()).map_err(|e| e.to_string()),
        Cmd::Render {
            path,
            out,
            metronome,
        } => {
            let s = render_with(&load(&path)?, &RenderOptions { metronome });
            std::fs::write(&out, to_wav_bytes(&s)).map_err(|e| e.to_string())?;
            Ok(json!({"ok": true, "out": out, "analysis": analyze(&s)}))
        }
        Cmd::Analyze { path } => Ok(
            json!({"ok": true, "analysis": analyze(&render_with(&load(&path)?, &RenderOptions::default()))}),
        ),
        Cmd::Diagnose { path } => {
            let p = load(&path)?;
            let report = kymion_audio::diagnose(&p);
            Ok(json!({"ok": true, "diagnostic": report}))
        }
        Cmd::ImportAudio {
            project,
            wav,
            track,
            start,
        } => {
            let bytes = std::fs::read(&wav).map_err(|e| format!("{}: {e}", wav.display()))?;
            let (sr, samples) = decode_wav(&bytes)?;
            let name = wav
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("audio")
                .to_string();
            let mut e = Engine::new(load(&project)?);
            let asset = e.import_audio(&name, sr, &samples);
            let track_id = match track {
                Some(t) => t,
                None => e
                    .apply(Command::AddTrack {
                        name: name.clone(),
                        kind: TrackKind::Audio,
                    })
                    .map_err(|er| er.to_string())?
                    .track_id
                    .unwrap(),
            };
            let o = e
                .apply(Command::AddClip {
                    track: track_id,
                    asset,
                    start,
                    gain: None,
                    offset_ticks: None,
                    duration_ticks: None,
                })
                .map_err(|er| er.to_string())?;
            std::fs::write(&project, e.project.to_json()).map_err(|e| e.to_string())?;
            Ok(
                json!({"ok": true, "asset_id": asset, "track_id": track_id, "clip_id": o.clip_id,
                      "duration_sec": samples.len() as f32 / sr as f32, "sample_rate": sr}),
            )
        }
        Cmd::ImportMidi { midi_path, out } => {
            let bytes =
                std::fs::read(&midi_path).map_err(|e| format!("{}: {e}", midi_path.display()))?;
            let project = import_midi(&bytes).map_err(|e| e.to_string())?;
            if let Some(out_p) = &out {
                std::fs::write(out_p, project.to_json()).map_err(|e| e.to_string())?;
            }
            Ok(json!({
                "ok": true,
                "name": project.name,
                "tempo": project.tempo,
                "tracks": project.tracks.len(),
                "total_notes": project.tracks.iter().map(|t| t.notes.len()).sum::<usize>(),
                "saved_to": out
            }))
        }
        Cmd::ExportMidi { project_path, out } => {
            let p = load(&project_path)?;
            let bytes = export_midi(&p).map_err(|e| e.to_string())?;
            std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
            Ok(json!({
                "ok": true,
                "out": out,
                "tracks": p.tracks.len(),
                "notes": p.tracks.iter().map(|t| t.notes.len()).sum::<usize>()
            }))
        }
        Cmd::ImportSoundfont {
            project,
            sf2,
            name,
            track,
            bank,
            preset,
        } => {
            let bytes = std::fs::read(&sf2).map_err(|e| format!("{}: {e}", sf2.display()))?;
            let sf_name = name.unwrap_or_else(|| {
                sf2.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("soundfont")
                    .to_string()
            });
            let presets = kymion_audio::list_soundfont_presets(&bytes);
            let mut e = Engine::new(load(&project)?);
            let asset_id = e.import_soundfont(&sf_name, &bytes);

            if let Some(track_id) = track {
                e.apply(Command::SetTrackSoundfont {
                    track: track_id,
                    soundfont: Some(kymion_core::SoundfontConfig {
                        asset: asset_id,
                        bank,
                        preset,
                    }),
                })
                .map_err(|er| er.to_string())?;
            }

            std::fs::write(&project, e.project.to_json()).map_err(|e| e.to_string())?;
            Ok(json!({
                "ok": true,
                "asset_id": asset_id,
                "name": sf_name,
                "bytes": bytes.len(),
                "presets_count": presets.len(),
                "assigned_track": track,
            }))
        }
    }
}

fn main() {
    match run(Cli::parse()) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            println!("{}", json!({"ok": false, "error": e}));
            std::process::exit(1);
        }
    }
}
