//! Standard MIDI File (.mid) import and export using `midly`.

use crate::{Error, Note, Project, Result, Track, TrackKind, Waveform, MAX_TEMPO, MIN_TEMPO, PPQ};
use midly::{
    num::{u24, u28, u4, u7},
    Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
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

    // Tempo / time signature: use the earliest event in the file (ties go to the
    // first track), not whichever track happens to be scanned last.
    let mut tempo: Option<(u64, u32)> = None;
    let mut time_sig: Option<(u64, u8, u8)> = None;
    for track in &smf.tracks {
        let mut abs = 0u64;
        for event in track {
            abs += event.delta.as_int() as u64;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(t))
                    if t.as_int() > 0 && tempo.is_none_or(|(at, _)| abs < at) =>
                {
                    tempo = Some((abs, t.as_int()));
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d_pow, _, _))
                    if (1..=32).contains(&n) && time_sig.is_none_or(|(at, _, _)| abs < at) =>
                {
                    time_sig = Some((abs, n, 1u8 << d_pow.min(4)));
                }
                _ => {}
            }
        }
    }
    if let Some((_, us)) = tempo {
        project.tempo = (60_000_000.0 / us as f32).clamp(MIN_TEMPO, MAX_TEMPO);
    }
    if let Some((_, n, den)) = time_sig {
        project.time_sig_num = n;
        project.time_sig_den = den;
    }

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
                TrackEventKind::Meta(MetaMessage::Lyric(l_bytes))
                | TrackEventKind::Meta(MetaMessage::Text(l_bytes)) => {
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

        // Assign project-unique note IDs (shared counter with tracks) and attach lyrics
        for n in notes.iter_mut() {
            n.id = project.alloc();

            // Find closest lyric within 120 ticks
            if let Some((&lyric_tick, lyric_str)) = lyrics
                .iter()
                .find(|(&lt, _)| (lt as i64 - n.start as i64).abs() <= 120)
            {
                n.lyric = Some(lyric_str.clone());
                lyrics.remove(&lyric_tick);
            }
        }

        let kind = if is_vocal
            || track_name.to_lowercase().contains("vocal")
            || track_name.to_lowercase().contains("voice")
        {
            TrackKind::Vocal {
                voice: "default".into(),
            }
        } else {
            TrackKind::instrument(Waveform::Pluck)
        };

        let track_id = project.alloc();
        let mut trk = Track::new(track_id, track_name, kind);
        trk.notes = notes;
        project.tracks.push(trk);
    }

    if project.tracks.is_empty() {
        return Err(Error::Invalid("MIDI file contained no note data".into()));
    }

    Ok(project)
}

/// MIDI channel for the `index`-th track. Channel 10 (index 9) is General MIDI percussion,
/// so melodic tracks skip it: 0..=8, then 10..=15 (tracks beyond 15 share channels).
fn midi_channel(index: usize) -> u4 {
    let c = (index % 15) as u8;
    u4::from(if c >= 9 { c + 1 } else { c })
}

/// Export a Kymion Project to Standard MIDI File (.mid) bytes.
pub fn export_midi(project: &Project) -> Result<Vec<u8>> {
    let header = Header::new(
        Format::Parallel,
        Timing::Metrical(midly::num::u15::from(PPQ as u16)),
    );
    let mut smf = Smf::new(header);

    // Tempo track (Track 0)
    let us_per_beat = (60_000_000.0 / project.tempo.clamp(20.0, 400.0)) as u32;
    let tempo_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(us_per_beat))),
    };
    let time_sig_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::TimeSignature(
            project.time_sig_num,
            project.time_sig_den.max(1).trailing_zeros() as u8,
            24,
            8,
        )),
    };
    let end_event = TrackEvent {
        delta: u28::from(0u32),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    };
    smf.tracks
        .push(vec![tempo_event, time_sig_event, end_event]);

    // Data tracks
    for (t_idx, track) in project.tracks.iter().enumerate() {
        let channel = midi_channel(t_idx);
        let mut events: Vec<(u32, TrackEventKind)> = Vec::new();

        // Track Name
        events.push((
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(track.name.as_bytes())),
        ));

        for note in &track.notes {
            let key = u7::from(note.pitch.min(127));
            let vel = u7::from(note.velocity.clamp(1, 127));

            // Optional lyric
            if let Some(l) = &note.lyric {
                events.push((
                    note.start,
                    TrackEventKind::Meta(MetaMessage::Lyric(l.as_bytes())),
                ));
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
                note.start.saturating_add(note.duration),
                TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOff {
                        key,
                        vel: u7::from(0u8),
                    },
                },
            ));
        }

        // Sort chronologically; at equal ticks NoteOff goes first so a note that ends where
        // the next begins is released before the retrigger. A plain key (false < true)
        // keeps this a total order, and the sort is stable for everything else.
        events.sort_by_key(|(tick, kind)| {
            (
                *tick,
                !matches!(
                    kind,
                    TrackEventKind::Midi {
                        message: MidiMessage::NoteOff { .. },
                        ..
                    }
                ),
            )
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
    smf.write(&mut buf)
        .map_err(|e| Error::Invalid(format!("MIDI export error: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_roundtrip() {
        let mut p = Project::new("MIDI Test");
        p.tempo = 140.0;
        let mut t = Track::new(
            1,
            "Lead",
            TrackKind::Vocal {
                voice: "default".into(),
            },
        );
        t.notes = vec![
            Note {
                id: 1,
                start: 0,
                duration: 480,
                pitch: 60,
                velocity: 100,
                lyric: Some("Do".into()),
                expression: None,
            },
            Note {
                id: 2,
                start: 480,
                duration: 480,
                pitch: 62,
                velocity: 110,
                lyric: Some("Re".into()),
                expression: None,
            },
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

    fn two_track_tempo_file(first: (u32, f32), second: (u32, f32)) -> Vec<u8> {
        use midly::num::u15;
        let mut smf = Smf::new(Header::new(
            Format::Parallel,
            Timing::Metrical(u15::from(480)),
        ));
        for (tick, bpm) in [first, second] {
            let us = (60_000_000.0 / bpm) as u32;
            smf.tracks.push(vec![
                TrackEvent {
                    delta: u28::from(tick),
                    kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(us))),
                },
                TrackEvent {
                    delta: u28::from(0u32),
                    kind: TrackEventKind::Midi {
                        channel: u4::from(0u8),
                        message: MidiMessage::NoteOn {
                            key: u7::from(60u8),
                            vel: u7::from(90u8),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::from(480u32),
                    kind: TrackEventKind::Midi {
                        channel: u4::from(0u8),
                        message: MidiMessage::NoteOff {
                            key: u7::from(60u8),
                            vel: u7::from(0u8),
                        },
                    },
                },
                TrackEvent {
                    delta: u28::from(0u32),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                },
            ]);
        }
        let mut buf = Vec::new();
        smf.write(&mut buf).unwrap();
        buf
    }

    #[test]
    fn import_uses_earliest_tempo_event_not_last_track() {
        // both at tick 0: the first track wins
        let p = import_midi(&two_track_tempo_file((0, 100.0), (0, 150.0))).unwrap();
        assert!((p.tempo - 100.0).abs() < 0.1, "{}", p.tempo);
        // the second track's tempo comes first in time: it wins
        let p = import_midi(&two_track_tempo_file((960, 100.0), (0, 150.0))).unwrap();
        assert!((p.tempo - 150.0).abs() < 0.1, "{}", p.tempo);
    }

    #[test]
    fn imported_ids_are_unique_and_do_not_collide_with_new_ids() {
        let mut p = Project::new("ids");
        for n in 0..3 {
            let tid = p.alloc();
            let mut t = Track::new(tid, format!("T{n}"), TrackKind::instrument(Waveform::Sine));
            for k in 0..2 {
                let nid = p.alloc();
                t.notes.push(Note {
                    id: nid,
                    start: k * 480,
                    duration: 240,
                    pitch: 60 + k as u8,
                    velocity: 100,
                    lyric: None,
                    expression: None,
                });
            }
            p.tracks.push(t);
        }
        let bytes = export_midi(&p).unwrap();
        let mut e = crate::Engine::new(import_midi(&bytes).unwrap());

        let mut ids: Vec<u32> = e.project.tracks.iter().map(|t| t.id).collect();
        ids.extend(
            e.project
                .tracks
                .iter()
                .flat_map(|t| t.notes.iter().map(|n| n.id)),
        );
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "duplicate ids after import: {ids:?}"
        );

        // ids handed out afterwards must not collide with anything imported
        let new_id = e
            .apply(crate::Command::AddTrack {
                name: "new".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        assert!(
            !ids.contains(&new_id),
            "new id {new_id} collides with {ids:?}"
        );
    }

    fn chord_project(tracks: usize) -> Project {
        let mut p = Project::new("ch");
        for n in 0..tracks {
            let tid = p.alloc();
            let mut t = Track::new(tid, format!("T{n}"), TrackKind::instrument(Waveform::Sine));
            let nid = p.alloc();
            t.notes.push(Note {
                id: nid,
                start: 0,
                duration: 480,
                pitch: 60,
                velocity: 100,
                lyric: None,
                expression: None,
            });
            p.tracks.push(t);
        }
        p
    }

    #[test]
    fn export_never_uses_the_percussion_channel() {
        for i in 0..64 {
            let ch = midi_channel(i).as_int();
            assert!(ch != 9 && ch <= 15, "track {i} -> channel {ch}");
        }
        let bytes = export_midi(&chord_project(20)).unwrap();
        let smf = Smf::parse(&bytes).unwrap();
        let mut used = std::collections::BTreeSet::new();
        for track in &smf.tracks {
            for e in track {
                if let TrackEventKind::Midi { channel, .. } = e.kind {
                    used.insert(channel.as_int());
                }
            }
        }
        assert!(!used.contains(&9), "channels used: {used:?}");
        assert_eq!(used.len(), 15);
    }

    #[test]
    fn export_releases_a_note_before_retriggering_the_same_pitch() {
        let mut p = Project::new("legato");
        let tid = p.alloc();
        let mut t = Track::new(tid, "T", TrackKind::instrument(Waveform::Sine));
        for (i, start) in [0u32, 480].into_iter().enumerate() {
            t.notes.push(Note {
                id: 10 + i as u32,
                start,
                duration: 480,
                pitch: 60,
                velocity: 100,
                lyric: None,
                expression: None,
            });
        }
        p.tracks.push(t);
        let smf_bytes = export_midi(&p).unwrap();
        let smf = Smf::parse(&smf_bytes).unwrap();
        let mut tick = 0u32;
        let mut at_480 = vec![];
        for e in &smf.tracks[1] {
            tick += e.delta.as_int();
            if let (480, TrackEventKind::Midi { message, .. }) = (tick, e.kind) {
                at_480.push(matches!(message, MidiMessage::NoteOff { .. }));
            }
        }
        assert_eq!(
            at_480,
            vec![true, false],
            "NoteOff must precede NoteOn at tick 480"
        );
    }
}
