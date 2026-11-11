//! Kymion core: project model + JSON command protocol with undo/redo.
//!
//! Every mutation goes through [`Command`] so that humans (GUI), scripts and
//! AI agents (CLI/WASM) all share one serialisable, validated interface.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Ticks per quarter note.
pub const PPQ: u32 = 480;

#[derive(Debug, Error)]
pub enum Error {
    #[error("track {0} not found")]
    TrackNotFound(u32),
    #[error("note {0} not found")]
    NoteNotFound(u32),
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("nothing to {0}")]
    NothingTo(&'static str),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Waveform {
    Sine,
    Saw,
    Square,
    Triangle,
    #[default]
    Pluck,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackKind {
    Instrument { waveform: Waveform },
    /// Singing-voice track (vocaloid-style). Notes carry lyrics.
    Vocal { voice: String },
}

/// Vocal expression parameters (per-track defaults, per-note override).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Expression {
    /// Vibrato depth in cents.
    pub vibrato_depth: f32,
    /// Vibrato rate in Hz.
    pub vibrato_rate: f32,
    /// Vibrato onset delay in seconds.
    pub vibrato_delay: f32,
    /// Portamento time between notes in seconds.
    pub portamento: f32,
    /// Breathiness 0..1.
    pub breath: f32,
    /// Formant shift in semitones (gender factor).
    pub formant_shift: f32,
}
impl Default for Expression {
    fn default() -> Self {
        Self { vibrato_depth: 30.0, vibrato_rate: 5.5, vibrato_delay: 0.25, portamento: 0.04, breath: 0.1, formant_shift: 0.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Note {
    pub id: u32,
    pub start: u32,
    pub duration: u32,
    /// MIDI note number (60 = C4).
    pub pitch: u8,
    pub velocity: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Track {
    pub id: u32,
    pub name: String,
    pub kind: TrackKind,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub expression: Expression,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo: f32,
    pub tracks: Vec<Track>,
    next_id: u32,
}

impl Default for Project {
    fn default() -> Self {
        Self { name: "Untitled".into(), sample_rate: 44100, tempo: 120.0, tracks: vec![], next_id: 1 }
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Default::default() }
    }
    pub fn ticks_to_secs(&self, ticks: u32) -> f32 {
        ticks as f32 * 60.0 / (self.tempo * PPQ as f32)
    }
    pub fn length_ticks(&self) -> u32 {
        self.tracks.iter().flat_map(|t| &t.notes).map(|n| n.start + n.duration).max().unwrap_or(0)
    }
    pub fn track(&self, id: u32) -> Result<&Track> {
        self.tracks.iter().find(|t| t.id == id).ok_or(Error::TrackNotFound(id))
    }
    fn track_mut(&mut self, id: u32) -> Result<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id).ok_or(Error::TrackNotFound(id))
    }
    fn alloc(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("project serialises")
    }
}

/// All edit operations. JSON form: `{"cmd":"add_note","track":1,...}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    SetTempo { bpm: f32 },
    SetName { name: String },
    AddTrack { name: String, kind: TrackKind },
    RemoveTrack { track: u32 },
    SetMix { track: u32, gain: Option<f32>, pan: Option<f32>, mute: Option<bool> },
    AddNote { track: u32, start: u32, duration: u32, pitch: u8, #[serde(default = "vel")] velocity: u8, lyric: Option<String> },
    RemoveNote { track: u32, note: u32 },
    MoveNote { track: u32, note: u32, start: Option<u32>, pitch: Option<u8>, duration: Option<u32> },
    /// Assign lyrics to consecutive notes of a vocal track (one syllable per note).
    SetLyrics { track: u32, lyrics: Vec<String> },
    /// Tune expression for a whole track (`note` = None) or a single note.
    SetExpression { track: u32, note: Option<u32>, expression: Expression },
    /// Quantize note starts to a grid (ticks).
    Quantize { track: u32, grid: u32 },
    Transpose { track: u32, semitones: i8 },
}
fn vel() -> u8 { 100 }

/// Result of applying a command (ids created, etc.), JSON-friendly for agents.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Outcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_id: Option<u32>,
}

/// Project + undo/redo history.
#[derive(Default)]
pub struct Engine {
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl Engine {
    pub fn new(project: Project) -> Self {
        Self { project, ..Default::default() }
    }

    pub fn apply_json(&mut self, json: &str) -> Result<Outcome> {
        let cmd: Command = serde_json::from_str(json)?;
        self.apply(cmd)
    }

    pub fn apply(&mut self, cmd: Command) -> Result<Outcome> {
        let before = self.project.clone();
        match apply_inner(&mut self.project, cmd) {
            Ok(o) => {
                self.undo.push(before);
                self.redo.clear();
                Ok(o)
            }
            Err(e) => {
                self.project = before;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Result<()> {
        let prev = self.undo.pop().ok_or(Error::NothingTo("undo"))?;
        self.redo.push(std::mem::replace(&mut self.project, prev));
        Ok(())
    }
    pub fn redo(&mut self) -> Result<()> {
        let next = self.redo.pop().ok_or(Error::NothingTo("redo"))?;
        self.undo.push(std::mem::replace(&mut self.project, next));
        Ok(())
    }
}

fn apply_inner(p: &mut Project, cmd: Command) -> Result<Outcome> {
    let mut out = Outcome { ok: true, ..Default::default() };
    match cmd {
        Command::SetTempo { bpm } => {
            if !(20.0..=400.0).contains(&bpm) {
                return Err(Error::Invalid("bpm must be 20..400".into()));
            }
            p.tempo = bpm;
        }
        Command::SetName { name } => p.name = name,
        Command::AddTrack { name, kind } => {
            let id = p.alloc();
            p.tracks.push(Track { id, name, kind, gain: 0.8, pan: 0.0, mute: false, expression: Expression::default(), notes: vec![] });
            out.track_id = Some(id);
        }
        Command::RemoveTrack { track } => {
            p.track(track)?;
            p.tracks.retain(|t| t.id != track);
        }
        Command::SetMix { track, gain, pan, mute } => {
            let t = p.track_mut(track)?;
            if let Some(g) = gain { t.gain = g.clamp(0.0, 2.0); }
            if let Some(v) = pan { t.pan = v.clamp(-1.0, 1.0); }
            if let Some(m) = mute { t.mute = m; }
        }
        Command::AddNote { track, start, duration, pitch, velocity, lyric } => {
            if duration == 0 || pitch > 127 || velocity > 127 {
                return Err(Error::Invalid("duration>0, pitch/velocity 0..127".into()));
            }
            let id = p.alloc();
            let t = p.track_mut(track)?;
            t.notes.push(Note { id, start, duration, pitch, velocity, lyric, expression: None });
            t.notes.sort_by_key(|n| (n.start, n.pitch));
            out.note_id = Some(id);
        }
        Command::RemoveNote { track, note } => {
            let t = p.track_mut(track)?;
            let n = t.notes.len();
            t.notes.retain(|x| x.id != note);
            if t.notes.len() == n { return Err(Error::NoteNotFound(note)); }
        }
        Command::MoveNote { track, note, start, pitch, duration } => {
            let t = p.track_mut(track)?;
            let n = t.notes.iter_mut().find(|x| x.id == note).ok_or(Error::NoteNotFound(note))?;
            if let Some(s) = start { n.start = s; }
            if let Some(v) = pitch { if v > 127 { return Err(Error::Invalid("pitch".into())); } n.pitch = v; }
            if let Some(d) = duration { if d == 0 { return Err(Error::Invalid("duration".into())); } n.duration = d; }
            t.notes.sort_by_key(|n| (n.start, n.pitch));
        }
        Command::SetLyrics { track, lyrics } => {
            let t = p.track_mut(track)?;
            for (n, l) in t.notes.iter_mut().zip(lyrics) { n.lyric = Some(l); }
        }
        Command::SetExpression { track, note, expression } => {
            let t = p.track_mut(track)?;
            match note {
                None => t.expression = expression,
                Some(id) => t.notes.iter_mut().find(|x| x.id == id).ok_or(Error::NoteNotFound(id))?.expression = Some(expression),
            }
        }
        Command::Quantize { track, grid } => {
            if grid == 0 { return Err(Error::Invalid("grid>0".into())); }
            let t = p.track_mut(track)?;
            for n in &mut t.notes { n.start = (n.start + grid / 2) / grid * grid; }
            t.notes.sort_by_key(|n| (n.start, n.pitch));
        }
        Command::Transpose { track, semitones } => {
            let t = p.track_mut(track)?;
            for n in &mut t.notes {
                n.pitch = (n.pitch as i16 + semitones as i16).clamp(0, 127) as u8;
            }
        }
    }
    Ok(out)
}

/// JSON Schema of the command protocol (for agent tool definitions).
pub fn command_schema() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(Command)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_undo_redo() {
        let mut e = Engine::default();
        let o = e.apply_json(r#"{"cmd":"add_track","name":"Lead","kind":{"type":"vocal","voice":"default"}}"#).unwrap();
        let t = o.track_id.unwrap();
        e.apply(Command::AddNote { track: t, start: 0, duration: 480, pitch: 60, velocity: 100, lyric: Some("la".into()) }).unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 1);
        e.undo().unwrap();
        assert!(e.project.tracks[0].notes.is_empty());
        e.redo().unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 1);
    }

    #[test]
    fn invalid_leaves_state() {
        let mut e = Engine::default();
        assert!(e.apply(Command::SetTempo { bpm: 5.0 }).is_err());
        assert_eq!(e.project.tempo, 120.0);
    }
}
