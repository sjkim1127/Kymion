//! Standard MIDI File (.mid) import and export using `midly`.

use crate::{Error, Note, Project, Result, Track, TrackKind, Waveform, PPQ};
use midly::{
    Format, Header, MetaMessage, MidiMessage, num::{u4, u7, u24, u28},
    Smf, Timing, TrackEvent, TrackEventKind,
};
use std::collections::HashMap;

/// Import a Standard MIDI File (.mid) into a Kymion Project.
pub fn import_midi(bytes: &[u8]) -> Result<Project> {
    let smf = Smf::parse(bytes).map_err(|e| Error::Invalid(format!("MIDI parse error: {e}")))?;

    let mut project = Project::new("Imported MIDI");

    // Convert timing / ticks per beat to Kymion PPQ (480)
    let file_ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as f32,
        Timing::Timecode(_, _) => 480.0,
    };
    let tick_scale = PPQ as f32 / file_ppq.max(1.0);

    // Scan tempo first
    for track in &smf.tracks {
        for event in track {
            if let TrackEventKind::Meta(MetaMessage::Tempo(t)) = event.kind {
                let us_per_beat = t.as_int();
                if us_per_beat > 0 {
                    let bpm = 60_000_000.0 / us_per_beat as f32;
                    project.tempo = bpm.clamp(20.0, 400.0);
                    break;
                }
            }
        }
    }

    for track in &smf.tracks {
        for event in track {
            if let TrackEventKind::Meta(MetaMessage::TimeSignature(n, d_pow, _, _)) = event.kind {
                let den = 1u8 << d_pow.min(4);
                if n > 0 && n <= 32 {
                    project.time_sig_num = n;
                    project.time_sig_den = den;
                }
            }
        }
    }

    let mut track_id = 1u32;

    for (t_idx, smf_track) in smf.tracks.iter().enumerate() {
        let mut cur_tick = 0u64;
        let mut track_name = format!("Track {}", t_idx + 1);
        let mut open_notes: HashMap<(u8, u8), (u32, u8)> = HashMap::new(); // (channel, key) -> (start_tick, velocity)
        let mut notes: Vec<Note> = Vec::new();
        let mut lyrics: HashMap<u32, String> = HashMap::new();
        let mut is_vocal = false;

        for event in smf_track {
            cur_tick += event.delta.as_int() as u64;
            let tick = (cur_tick as f32 * tick_scale).round() as u32;

            match event.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(name_bytes)) => {
                    if let Ok(s) = std::str::from_utf8(name_bytes) {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            track_name = trimmed.to_string();
                        }
                    }
                }
                TrackEventKind::Meta(MetaMessage::Lyric(l_bytes)) | TrackEventKind::Meta(MetaMessage::Text(l_bytes)) => {
                    if let Ok(s) = std::str::from_utf8(l_bytes) {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            lyrics.insert(tick, trimmed.to_string());
                            is_vocal = true;
                        }
                    }
                }
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } => {
                            let k = key.as_int();
                            let v = vel.as_int();
                            if v > 0 {
                                open_notes.insert((ch, k), (tick, v));
                            } else if let Some((start, orig_vel)) = open_notes.remove(&(ch, k)) {
                                let duration = tick.saturating_sub(start).max(60);
                                notes.push(Note {
                                    id: 0,
                                    start,
                                    duration,
                                    pitch: k,
                                    velocity: orig_vel,
                                    lyric: None,
                                    expression: None,
                                });
                            }
                        }
                        MidiMessage::NoteOff { key, .. } => {
                            let k = key.as_int();
                            if let Some((start, orig_vel)) = open_notes.remove(&(ch, k)) {
                                let duration = tick.saturating_sub(start).max(60);
                                notes.push(Note {
                                    id: 0,
                                    start,
                                    duration,
                                    pitch: k,
                                    velocity: orig_vel,
                                    lyric: None,
                                    expression: None,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Close any dangling notes
        for ((_, k), (start, orig_vel)) in open_notes {
            notes.push(Note {
                id: 0,
                start,
                duration: 240,
                pitch: k,
                velocity: orig_vel,
                lyric: None,
                expression: None,
            });
        }

        if notes.is_empty() {
            continue;
        }

        notes.sort_by_key(|n| (n.start, n.pitch));

        // Assign note IDs and attach lyrics
        let mut next_note_id = 1u32;
        for n in &mut notes {
            n.id = next_note_id;
            next_note_id += 1;

            // Find closest lyric within 120 ticks
            if let Some((&lyric_tick, lyric_str)) = lyrics.iter().find(|(&lt, _)| (lt as i64 - n.start as i64).abs() <= 120) {
                n.lyric = Some(lyric_str.clone());
                lyrics.remove(&lyric_tick);
            }
        }

        let kind = if is_vocal || track_name.to_lowercase().contains("vocal") || track_name.to_lowercase().contains("voice") {
            TrackKind::Vocal { voice: "default".into() }
        } else {
            TrackKind::instrument(Waveform::Pluck)
        };

        let mut trk = Track::new(track_id, track_name, kind);
        trk.notes = notes;
        project.tracks.push(trk);

        track_id += 1;
    }

    if project.tracks.is_empty() {
        return Err(Error::Invalid("MIDI file contained no note data".into()));
    }

    Ok(project)
}

/// Export a Kymion Project to Standard MIDI File (.mid) bytes.
pub fn export_midi(project: &Project) -> Result<Vec<u8>> {
    let header = Header::new(Format::Parallel, Timing::Metrical(midly::num::u15::from(PPQ as u16)));
    let mut smf = Smf::new(header);

    // Tempo track (Track 0)
    let us_per_beat = (60_000_000.0 / project.tempo.clamp(20.0, 400.0)) as u32;
    let tempo_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(us_per_beat))),
    };
    let time_sig_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::TimeSignature(project.time_sig_num, project.time_sig_den.max(1).trailing_zeros() as u8, 24, 8)),
    };
    let end_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    };
    smf.tracks.push(vec![tempo_event, time_sig_event, end_event]);

    // Data tracks
    for (t_idx, track) in project.tracks.iter().enumerate() {
        let channel = u4::from((t_idx % 16) as u8);
        let mut events: Vec<(u32, TrackEventKind)> = Vec::new();

        // Track Name
        events.push((0, TrackEventKind::Meta(MetaMessage::TrackName(track.name.as_bytes()))));

        for note in &track.notes {
            let key = u7::from(note.pitch.min(127));
            let vel = u7::from(note.velocity.clamp(1, 127));

            // Optional lyric
            if let Some(l) = &note.lyric {
                events.push((note.start, TrackEventKind::Meta(MetaMessage::Lyric(l.as_bytes()))));
            }

            // Note On
            events.push((
                note.start,
                TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOn { key, vel },
                },
            ));

            // Note Off
            events.push((
                note.start + note.duration,
                TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOff { key, vel: u7::from(0u8) },
                },
            ));
        }

        // Sort events chronologically (NoteOff before NoteOn if simultaneous)
        events.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| match (&a.1, &b.1) {
                (TrackEventKind::Midi { message: MidiMessage::NoteOff { .. }, .. }, _) => std::cmp::Ordering::Less,
                (_, TrackEventKind::Midi { message: MidiMessage::NoteOff { .. }, .. }) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
        });

        // Convert absolute ticks to delta ticks
        let mut smf_track_events = Vec::new();
        let mut last_tick = 0u32;
        for (tick, kind) in events {
            let delta = u28::from(tick.saturating_sub(last_tick));
            last_tick = tick;
            smf_track_events.push(TrackEvent { delta, kind });
        }

        smf_track_events.push(TrackEvent {
            delta: u28::from(0u32),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });

        smf.tracks.push(smf_track_events);
    }

    let mut buf = Vec::new();
    smf.write(&mut buf).map_err(|e| Error::Invalid(format!("MIDI export error: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_roundtrip() {
        let mut p = Project::new("MIDI Test");
        p.tempo = 140.0;
        let mut t = Track::new(1, "Lead", TrackKind::Vocal { voice: "default".into() });
        t.notes = vec![
            Note { id: 1, start: 0, duration: 480, pitch: 60, velocity: 100, lyric: Some("Do".into()), expression: None },
            Note { id: 2, start: 480, duration: 480, pitch: 62, velocity: 110, lyric: Some("Re".into()), expression: None },
        ];
        p.tracks.push(t);

        let midi_bytes = export_midi(&p).expect("export should succeed");
        assert!(!midi_bytes.is_empty());

        let imported = import_midi(&midi_bytes).expect("import should succeed");
        assert!((imported.tempo - 140.0).abs() < 1e-2);
        assert_eq!(imported.tracks.len(), 1);
        assert_eq!(imported.tracks[0].notes.len(), 2);
        assert_eq!(imported.tracks[0].notes[0].pitch, 60);
        assert_eq!(imported.tracks[0].notes[1].pitch, 62);
    }
}
