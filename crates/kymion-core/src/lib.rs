//! Kymion core: project model + JSON command protocol with undo/redo.
//!
//! Every mutation goes through [`Command`] so that humans (GUI), scripts and
//! AI agents (CLI/WASM) all share one serialisable, validated interface.

pub mod midi;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Ticks per quarter note.
pub const PPQ: u32 = 480;
/// Accepted tempo range in BPM.
pub const MIN_TEMPO: f32 = 20.0;
pub const MAX_TEMPO: f32 = 400.0;
/// Accepted project sample-rate range in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 384_000;

/// Check a tempo in BPM (rejects NaN, zero, negative and out-of-range values).
pub fn validate_tempo(bpm: f32) -> Result<()> {
    if (MIN_TEMPO..=MAX_TEMPO).contains(&bpm) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "bpm must be {MIN_TEMPO}..{MAX_TEMPO}"
        )))
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("track {0} not found")]
    TrackNotFound(u32),
    #[error("note {0} not found")]
    NoteNotFound(u32),
    #[error("effect {0} not found")]
    FxNotFound(u32),
    #[error("clip {0} not found")]
    ClipNotFound(u32),
    #[error("asset {0} not found")]
    AssetNotFound(u32),
    #[error("soundfont {0} not found")]
    SoundfontNotFound(u32),
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
pub struct SoundfontConfig {
    pub asset: u32,
    #[serde(default)]
    pub bank: u16,
    #[serde(default)]
    pub preset: u16,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SynthFilter {
    #[default]
    Moog,
    Lowpass,
    Highpass,
    Bandpass,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SynthConfig {
    #[serde(default)]
    pub filter: SynthFilter,
    #[serde(default = "default_cutoff")]
    pub cutoff_hz: f32,
    #[serde(default)]
    pub resonance: f32,
    #[serde(default)]
    pub filter_env_amount: f32,
    #[serde(default = "default_attack")]
    pub attack: f32,
    #[serde(default = "default_decay")]
    pub decay: f32,
    #[serde(default = "default_sustain")]
    pub sustain: f32,
    #[serde(default = "default_release")]
    pub release: f32,
}

fn default_cutoff() -> f32 {
    20000.0
}
fn default_attack() -> f32 {
    0.005
}
fn default_decay() -> f32 {
    0.1
}
fn default_sustain() -> f32 {
    0.7
}
fn default_release() -> f32 {
    0.2
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            filter: SynthFilter::Moog,
            cutoff_hz: 20000.0,
            resonance: 0.0,
            filter_env_amount: 0.0,
            attack: 0.005,
            decay: 0.1,
            sustain: 0.7,
            release: 0.2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackKind {
    Instrument {
        #[serde(default)]
        waveform: Waveform,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        soundfont: Option<SoundfontConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        synth: Option<SynthConfig>,
    },
    /// Singing-voice track (vocaloid-style). Notes carry lyrics.
    Vocal { voice: String },
    /// Audio track holding sampled clips placed on the timeline.
    Audio,
}

impl TrackKind {
    pub fn instrument(waveform: Waveform) -> Self {
        Self::Instrument {
            waveform,
            soundfont: None,
            synth: None,
        }
    }

    pub fn soundfont(asset: u32, bank: u16, preset: u16) -> Self {
        Self::Instrument {
            waveform: Waveform::Pluck,
            soundfont: Some(SoundfontConfig {
                asset,
                bank,
                preset,
            }),
            synth: None,
        }
    }

    pub fn synth(waveform: Waveform, synth: SynthConfig) -> Self {
        Self::Instrument {
            waveform,
            soundfont: None,
            synth: Some(synth),
        }
    }
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
        Self {
            vibrato_depth: 30.0,
            vibrato_rate: 5.5,
            vibrato_delay: 0.25,
            portamento: 0.04,
            breath: 0.1,
            formant_shift: 0.0,
        }
    }
}

/// Audio effects supported by Kymion tracks and master bus.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FxKind {
    /// 3-band parametric EQ (Low Shelf, Mid Peaking, High Shelf)
    Eq {
        low_gain: f32,
        mid_gain: f32,
        mid_freq: f32,
        mid_q: f32,
        high_gain: f32,
    },
    /// Stereo Delay with feedback and ping-pong
    Delay {
        time_ms: f32,
        feedback: f32,
        mix: f32,
        ping_pong: bool,
    },
    /// Algorithmic stereo reverberation
    Reverb {
        room_size: f32,
        damping: f32,
        width: f32,
        mix: f32,
    },
    /// Dynamic range compressor
    Compressor {
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    },
}

impl Default for FxKind {
    fn default() -> Self {
        Self::Reverb {
            room_size: 0.6,
            damping: 0.3,
            width: 1.0,
            mix: 0.25,
        }
    }
}

/// Active effect instance in an FX chain.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FxInstance {
    pub id: u32,
    pub enabled: bool,
    pub kind: FxKind,
}

/// Mono audio sample data embedded in the project (base64 of i16 LE PCM).
///
/// `data` is reference-counted so that cloning a [`Project`] (undo snapshots,
/// GUI refreshes) never copies the embedded payload.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AudioAsset {
    pub id: u32,
    pub name: String,
    pub sample_rate: u32,
    pub data: Arc<str>,
}

impl AudioAsset {
    pub fn from_samples(id: u32, name: &str, sample_rate: u32, samples: &[f32]) -> Self {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        Self {
            id,
            name: name.into(),
            sample_rate,
            data: B64.encode(bytes).into(),
        }
    }
    /// Decode to f32 mono samples.
    pub fn samples(&self) -> Vec<f32> {
        let bytes = B64.decode(self.data.as_bytes()).unwrap_or_default();
        let (chunks, _) = bytes.as_chunks::<2>();
        chunks
            .iter()
            .map(|&[b0, b1]| i16::from_le_bytes([b0, b1]) as f32 / 32768.0)
            .collect()
    }
    pub fn duration_secs(&self) -> f32 {
        let n = B64
            .decode(self.data.as_bytes())
            .map(|b| b.len() / 2)
            .unwrap_or(0);
        n as f32 / self.sample_rate.max(1) as f32
    }
}

/// SoundFont 2 (.sf2) binary asset embedded in the project (base64).
/// `data` is reference-counted so project clones stay cheap.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SoundfontAsset {
    pub id: u32,
    pub name: String,
    pub data: Arc<str>,
}

impl SoundfontAsset {
    pub fn from_bytes(id: u32, name: &str, bytes: &[u8]) -> Self {
        Self {
            id,
            name: name.into(),
            data: B64.encode(bytes).into(),
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        B64.decode(self.data.as_bytes()).unwrap_or_default()
    }
}

/// An audio asset placed on an audio track.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AudioClip {
    pub id: u32,
    pub asset: u32,
    /// Start position in ticks.
    pub start: u32,
    #[serde(default = "default_gain")]
    pub gain: f32,
    /// Offset inside the audio asset in ticks (for trimming start).
    #[serde(default)]
    pub offset_ticks: u32,
    /// Length of the clip in ticks (None = play remainder of asset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ticks: Option<u32>,
    /// Fade-in length in ticks.
    #[serde(default)]
    pub fade_in_ticks: u32,
    /// Fade-out length in ticks.
    #[serde(default)]
    pub fade_out_ticks: u32,
}
fn default_gain() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoParam {
    Gain,
    Pan,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AutoPoint {
    pub tick: u32,
    pub value: f32,
}

/// Linear-interpolated automation lookup.
pub fn eval_auto(points: &[AutoPoint], tick: f32, default: f32) -> f32 {
    match points {
        [] => default,
        [only] => only.value,
        _ => {
            if tick <= points[0].tick as f32 {
                return points[0].value;
            }
            let last = &points[points.len() - 1];
            if tick >= last.tick as f32 {
                return last.value;
            }
            let i = points.partition_point(|p| (p.tick as f32) <= tick);
            let (a, b) = (&points[i - 1], &points[i]);
            let t = (tick - a.tick as f32) / (b.tick - a.tick).max(1) as f32;
            a.value + (b.value - a.value) * t
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LoopRegion {
    pub start: u32,
    pub end: u32,
}

/// Note payload for batch insertion.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NewNote {
    pub start: u32,
    pub duration: u32,
    pub pitch: u8,
    #[serde(default = "vel")]
    pub velocity: u8,
    #[serde(default)]
    pub lyric: Option<String>,
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
    #[serde(default)]
    pub solo: bool,
    pub expression: Expression,
    #[serde(default)]
    pub fx: Vec<FxInstance>,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub clips: Vec<AudioClip>,
    #[serde(default)]
    pub gain_auto: Vec<AutoPoint>,
    #[serde(default)]
    pub pan_auto: Vec<AutoPoint>,
}

impl Track {
    pub fn new(id: u32, name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            gain: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            expression: Expression::default(),
            fx: vec![],
            notes: vec![],
            clips: vec![],
            gain_auto: vec![],
            pan_auto: vec![],
        }
    }
}

/// Standard audio frequency bands for spectral & masking analysis.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyBand {
    SubBass,
    Bass,
    LowMid,
    Mid,
    HighMid,
    High,
}

impl FrequencyBand {
    pub const ALL: [FrequencyBand; 6] = [
        FrequencyBand::SubBass,
        FrequencyBand::Bass,
        FrequencyBand::LowMid,
        FrequencyBand::Mid,
        FrequencyBand::HighMid,
        FrequencyBand::High,
    ];

    pub fn freq_range_hz(&self) -> (f32, f32) {
        match self {
            FrequencyBand::SubBass => (20.0, 60.0),
            FrequencyBand::Bass => (60.0, 250.0),
            FrequencyBand::LowMid => (250.0, 500.0),
            FrequencyBand::Mid => (500.0, 2000.0),
            FrequencyBand::HighMid => (2000.0, 6000.0),
            FrequencyBand::High => (6000.0, 20000.0),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            FrequencyBand::SubBass => "Sub-Bass (20-60 Hz)",
            FrequencyBand::Bass => "Bass (60-250 Hz)",
            FrequencyBand::LowMid => "Low-Mid (250-500 Hz)",
            FrequencyBand::Mid => "Mid (500-2000 Hz)",
            FrequencyBand::HighMid => "High-Mid (2-6 kHz)",
            FrequencyBand::High => "High / Air (6-20 kHz)",
        }
    }
}

/// Severity classification of frequency masking between tracks.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClashSeverity {
    Low,
    Medium,
    High,
}

/// Detail of a frequency masking clash between two tracks.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FrequencyClash {
    pub track_a_id: u32,
    pub track_a_name: String,
    pub track_b_id: u32,
    pub track_b_name: String,
    pub band: FrequencyBand,
    pub freq_range_hz: (f32, f32),
    pub severity: ClashSeverity,
    pub overlap_score: f32,
    pub clash_duration_sec: f32,
    pub suggestion: String,
}

/// ITU-R BS.1770-4 standard loudness and true peak metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LufsReport {
    /// Integrated loudness across entire audio (ITU-R BS.1770-4 gated LKFS/LUFS)
    pub integrated: f32,
    /// Maximum momentary loudness (400ms window)
    pub momentary_max: f32,
    /// Maximum short-term loudness (3-second window)
    pub short_term_max: f32,
    /// Loudness Range in LU (statistical dynamic range)
    pub loudness_range: f32,
    /// True Peak level in dBTP
    pub true_peak_db: f32,
}

/// Overall mixing and acoustics diagnostic report for AI agents and producers.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DiagnosticReport {
    pub lufs: LufsReport,
    pub clashes: Vec<FrequencyClash>,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo: f32,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub master_fx: Vec<FxInstance>,
    #[serde(default = "four")]
    pub time_sig_num: u8,
    #[serde(default = "four")]
    pub time_sig_den: u8,
    #[serde(default)]
    pub loop_region: Option<LoopRegion>,
    #[serde(default)]
    pub assets: Vec<AudioAsset>,
    #[serde(default)]
    pub soundfonts: Vec<SoundfontAsset>,
    #[serde(default)]
    next_id: u32,
}
fn four() -> u8 {
    4
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            sample_rate: 44100,
            tempo: 120.0,
            tracks: vec![],
            master_fx: vec![],
            time_sig_num: 4,
            time_sig_den: 4,
            loop_region: None,
            assets: vec![],
            soundfonts: vec![],
            next_id: 1,
        }
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
    pub fn ticks_to_secs(&self, ticks: u32) -> f32 {
        ticks as f32 * 60.0 / (self.tempo * PPQ as f32)
    }
    pub fn secs_to_ticks(&self, secs: f32) -> u32 {
        (secs * self.tempo * PPQ as f32 / 60.0) as u32
    }
    /// Ticks per beat for the current time signature denominator.
    pub fn beat_ticks(&self) -> u32 {
        PPQ * 4 / self.time_sig_den.max(1) as u32
    }
    pub fn bar_ticks(&self) -> u32 {
        self.beat_ticks() * self.time_sig_num.max(1) as u32
    }
    pub fn clip_len_ticks(&self, clip: &AudioClip) -> u32 {
        if let Some(d) = clip.duration_ticks {
            return d;
        }
        self.assets
            .iter()
            .find(|a| a.id == clip.asset)
            .map(|a| {
                self.secs_to_ticks(a.duration_secs())
                    .saturating_sub(clip.offset_ticks)
            })
            .unwrap_or(0)
    }
    pub fn length_ticks(&self) -> u32 {
        let notes = self
            .tracks
            .iter()
            .flat_map(|t| &t.notes)
            .map(|n| n.start.saturating_add(n.duration))
            .max()
            .unwrap_or(0);
        let clips = self
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.start.saturating_add(self.clip_len_ticks(c)))
            .max()
            .unwrap_or(0);
        notes.max(clips)
    }
    pub fn track(&self, id: u32) -> Result<&Track> {
        self.tracks
            .iter()
            .find(|t| t.id == id)
            .ok_or(Error::TrackNotFound(id))
    }
    pub fn track_mut(&mut self, id: u32) -> Result<&mut Track> {
        self.tracks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Error::TrackNotFound(id))
    }
    pub fn alloc(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn from_json(s: &str) -> Result<Self> {
        let mut p: Self = serde_json::from_str(s)?;
        validate_tempo(p.tempo)?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&p.sample_rate) {
            return Err(Error::Invalid(format!(
                "sample_rate must be {MIN_SAMPLE_RATE}..{MAX_SAMPLE_RATE}"
            )));
        }
        p.fix_next_id();
        Ok(p)
    }
    /// Make sure the id counter is above every id already present, so ids
    /// handed out by [`Project::alloc`] can never collide with existing
    /// tracks, notes, clips, effects or assets (e.g. after importing a MIDI
    /// file or loading a hand-edited / older project file).
    pub fn fix_next_id(&mut self) {
        let mut max = 0u32;
        let fx = |l: &[FxInstance], m: &mut u32| {
            for f in l {
                *m = (*m).max(f.id);
            }
        };
        for t in &self.tracks {
            max = max.max(t.id);
            for n in &t.notes {
                max = max.max(n.id);
            }
            for c in &t.clips {
                max = max.max(c.id);
            }
            fx(&t.fx, &mut max);
        }
        fx(&self.master_fx, &mut max);
        for a in &self.assets {
            max = max.max(a.id);
        }
        for s in &self.soundfonts {
            max = max.max(s.id);
        }
        self.next_id = self.next_id.max(max.saturating_add(1)).max(1);
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("project serialises")
    }
    /// Compact JSON without embedded audio data (cheap for GUI refreshes).
    pub fn to_json_light(&self) -> String {
        let mut p = self.clone();
        for a in &mut p.assets {
            a.data = Arc::from("");
        }
        for sf in &mut p.soundfonts {
            sf.data = Arc::from("");
        }
        serde_json::to_string(&p).expect("project serialises")
    }
}

/// All edit operations. JSON form: `{"cmd":"add_note","track":1,...}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    SetTempo {
        bpm: f32,
    },
    SetName {
        name: String,
    },
    AddTrack {
        name: String,
        kind: TrackKind,
    },
    RemoveTrack {
        track: u32,
    },
    RenameTrack {
        track: u32,
        name: String,
    },
    SetMix {
        track: u32,
        gain: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
    /// Set time signature (denominator must be 1,2,4,8 or 16).
    SetTimeSignature {
        numerator: u8,
        denominator: u8,
    },
    /// Set the loop region in ticks; omit start/end to clear it.
    SetLoop {
        start: Option<u32>,
        end: Option<u32>,
    },
    /// Insert many notes atomically (single undo step). Returns `note_ids`.
    AddNotes {
        track: u32,
        notes: Vec<NewNote>,
    },
    RemoveNotes {
        track: u32,
        notes: Vec<u32>,
    },
    /// Move many notes by a tick/pitch delta (single undo step).
    MoveNotes {
        track: u32,
        notes: Vec<u32>,
        delta_tick: i32,
        delta_pitch: i8,
    },
    SetVelocity {
        track: u32,
        notes: Vec<u32>,
        velocity: u8,
    },
    /// Split a note at an absolute tick strictly inside it.
    SplitNote {
        track: u32,
        note: u32,
        at: u32,
    },
    /// Add/replace an automation point for gain (0..2) or pan (-1..1).
    AddAutoPoint {
        track: u32,
        param: AutoParam,
        tick: u32,
        value: f32,
    },
    ClearAuto {
        track: u32,
        param: AutoParam,
    },
    /// Place an imported audio asset on an audio track.
    AddClip {
        track: u32,
        asset: u32,
        start: u32,
        #[serde(default)]
        gain: Option<f32>,
        #[serde(default)]
        offset_ticks: Option<u32>,
        #[serde(default)]
        duration_ticks: Option<u32>,
    },
    RemoveClip {
        track: u32,
        clip: u32,
    },
    MoveClip {
        track: u32,
        clip: u32,
        start: u32,
    },
    /// Trim an audio clip's start, offset into the asset, or duration.
    TrimClip {
        track: u32,
        clip: u32,
        #[serde(default)]
        start: Option<u32>,
        #[serde(default)]
        offset_ticks: Option<u32>,
        #[serde(default)]
        duration_ticks: Option<u32>,
    },
    /// Split an audio clip at an absolute timeline tick inside the clip.
    SplitClip {
        track: u32,
        clip: u32,
        at: u32,
    },
    /// Set the gain multiplier (0..2) of an audio clip.
    SetClipGain {
        track: u32,
        clip: u32,
        gain: f32,
    },
    /// Set fade-in and/or fade-out lengths in ticks for an audio clip.
    SetClipFade {
        track: u32,
        clip: u32,
        #[serde(default)]
        fade_in_ticks: Option<u32>,
        #[serde(default)]
        fade_out_ticks: Option<u32>,
    },
    AddNote {
        track: u32,
        start: u32,
        duration: u32,
        pitch: u8,
        #[serde(default = "vel")]
        velocity: u8,
        lyric: Option<String>,
    },
    RemoveNote {
        track: u32,
        note: u32,
    },
    MoveNote {
        track: u32,
        note: u32,
        start: Option<u32>,
        pitch: Option<u8>,
        duration: Option<u32>,
    },
    /// Assign lyrics to consecutive notes of a vocal track (one syllable per note).
    SetLyrics {
        track: u32,
        lyrics: Vec<String>,
    },
    /// Tune expression for a whole track (`note` = None) or a single note.
    SetExpression {
        track: u32,
        note: Option<u32>,
        expression: Expression,
    },
    /// Quantize note starts to a grid (ticks).
    Quantize {
        track: u32,
        grid: u32,
    },
    Transpose {
        track: u32,
        semitones: i8,
    },
    /// Add an effect to a track FX chain or master bus (`track` = None).
    AddFx {
        track: Option<u32>,
        kind: FxKind,
    },
    /// Modify effect parameters or toggle bypass.
    SetFx {
        track: Option<u32>,
        fx: u32,
        kind: Option<FxKind>,
        enabled: Option<bool>,
    },
    /// Remove an effect from an FX chain.
    RemoveFx {
        track: Option<u32>,
        fx: u32,
    },
    /// Add a SoundFont 2 binary asset (base64-encoded).
    AddSoundfont {
        name: String,
        data: String,
    },
    /// Configure SoundFont instrument on an instrument track.
    SetTrackSoundfont {
        track: u32,
        soundfont: Option<SoundfontConfig>,
    },
    /// Set oscillator waveform on an instrument track.
    SetTrackWaveform {
        track: u32,
        waveform: Waveform,
    },
    /// Configure subtractive synthesizer engine (filter, resonance, ADSR) on an instrument track.
    SetTrackSynth {
        track: u32,
        synth: Option<SynthConfig>,
    },
}
fn vel() -> u8 {
    100
}

/// A note must end at a tick representable as u32 (`start + duration`).
fn check_tick_range(start: u32, duration: u32) -> Result<()> {
    start
        .checked_add(duration)
        .map(|_| ())
        .ok_or_else(|| Error::Invalid("start + duration exceeds the maximum tick".into()))
}

/// Result of applying a command (ids created, etc.), JSON-friendly for agents.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Outcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fx_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub note_ids: Vec<u32>,
}

/// Project + undo/redo history.
#[derive(Default)]
pub struct Engine {
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl Engine {
    pub fn new(mut project: Project) -> Self {
        project.fix_next_id();
        Self {
            project,
            ..Default::default()
        }
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

    /// Embed mono samples as a project asset (undoable). Returns the asset id.
    pub fn import_audio(&mut self, name: &str, sample_rate: u32, samples: &[f32]) -> u32 {
        self.undo.push(self.project.clone());
        self.redo.clear();
        let id = self.project.alloc();
        self.project
            .assets
            .push(AudioAsset::from_samples(id, name, sample_rate, samples));
        id
    }

    /// Embed SoundFont 2 (.sf2) binary as a project asset (undoable). Returns the asset id.
    pub fn import_soundfont(&mut self, name: &str, bytes: &[u8]) -> u32 {
        self.undo.push(self.project.clone());
        self.redo.clear();
        let id = self.project.alloc();
        self.project
            .soundfonts
            .push(SoundfontAsset::from_bytes(id, name, bytes));
        id
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
    let mut out = Outcome {
        ok: true,
        ..Default::default()
    };
    match cmd {
        Command::SetTempo { bpm } => {
            validate_tempo(bpm)?;
            p.tempo = bpm;
        }
        Command::SetName { name } => p.name = name,
        Command::AddTrack { name, kind } => {
            let id = p.alloc();
            p.tracks.push(Track::new(id, name, kind));
            out.track_id = Some(id);
        }
        Command::RemoveTrack { track } => {
            p.track(track)?;
            p.tracks.retain(|t| t.id != track);
        }
        Command::SetMix {
            track,
            gain,
            pan,
            mute,
            solo,
        } => {
            let t = p.track_mut(track)?;
            if let Some(v) = solo {
                t.solo = v;
            }
            if let Some(g) = gain {
                t.gain = g.clamp(0.0, 2.0);
            }
            if let Some(v) = pan {
                t.pan = v.clamp(-1.0, 1.0);
            }
            if let Some(m) = mute {
                t.mute = m;
            }
        }
        Command::AddNote {
            track,
            start,
            duration,
            pitch,
            velocity,
            lyric,
        } => {
            if duration == 0 || pitch > 127 || velocity > 127 {
                return Err(Error::Invalid("duration>0, pitch/velocity 0..127".into()));
            }
            check_tick_range(start, duration)?;
            let id = p.alloc();
            let t = p.track_mut(track)?;
            t.notes.push(Note {
                id,
                start,
                duration,
                pitch,
                velocity,
                lyric,
                expression: None,
            });
            t.notes.sort_by_key(|n| (n.start, n.pitch));
            out.note_id = Some(id);
        }
        Command::RemoveNote { track, note } => {
            let t = p.track_mut(track)?;
            let n = t.notes.len();
            t.notes.retain(|x| x.id != note);
            if t.notes.len() == n {
                return Err(Error::NoteNotFound(note));
            }
        }
        Command::MoveNote {
            track,
            note,
            start,
            pitch,
            duration,
        } => {
            let t = p.track_mut(track)?;
            let n = t
                .notes
                .iter_mut()
                .find(|x| x.id == note)
                .ok_or(Error::NoteNotFound(note))?;
            if let Some(s) = start {
                n.start = s;
            }
            if let Some(v) = pitch {
                if v > 127 {
                    return Err(Error::Invalid("pitch".into()));
                }
                n.pitch = v;
            }
            if let Some(d) = duration {
                if d == 0 {
                    return Err(Error::Invalid("duration".into()));
                }
                n.duration = d;
            }
            check_tick_range(n.start, n.duration)?;
            t.notes.sort_by_key(|n| (n.start, n.pitch));
        }
        Command::SetLyrics { track, lyrics } => {
            let t = p.track_mut(track)?;
            for (n, l) in t.notes.iter_mut().zip(lyrics) {
                n.lyric = Some(l);
            }
        }
        Command::SetExpression {
            track,
            note,
            expression,
        } => {
            let t = p.track_mut(track)?;
            match note {
                None => t.expression = expression,
                Some(id) => {
                    t.notes
                        .iter_mut()
                        .find(|x| x.id == id)
                        .ok_or(Error::NoteNotFound(id))?
                        .expression = Some(expression)
                }
            }
        }
        Command::Quantize { track, grid } => {
            if grid == 0 {
                return Err(Error::Invalid("grid>0".into()));
            }
            let t = p.track_mut(track)?;
            for n in &mut t.notes {
                let q = n.start.saturating_add(grid / 2) / grid * grid;
                // rounding up must not push the end past the u32 tick range
                n.start = q.min(u32::MAX - n.duration);
            }
            t.notes.sort_by_key(|n| (n.start, n.pitch));
        }
        Command::Transpose { track, semitones } => {
            let t = p.track_mut(track)?;
            for n in &mut t.notes {
                n.pitch = (n.pitch as i16 + semitones as i16).clamp(0, 127) as u8;
            }
        }
        Command::RenameTrack { track, name } => p.track_mut(track)?.name = name,
        Command::SetTimeSignature {
            numerator,
            denominator,
        } => {
            if numerator == 0 || numerator > 32 || ![1, 2, 4, 8, 16].contains(&denominator) {
                return Err(Error::Invalid(
                    "numerator 1..32, denominator 1/2/4/8/16".into(),
                ));
            }
            p.time_sig_num = numerator;
            p.time_sig_den = denominator;
        }
        Command::SetLoop { start, end } => match (start, end) {
            (Some(s), Some(e)) if e > s => p.loop_region = Some(LoopRegion { start: s, end: e }),
            (None, None) => p.loop_region = None,
            _ => {
                return Err(Error::Invalid(
                    "loop needs start<end, or both omitted to clear".into(),
                ))
            }
        },
        Command::AddNotes { track, notes } => {
            for n in &notes {
                if n.duration == 0 || n.pitch > 127 || n.velocity > 127 {
                    return Err(Error::Invalid("duration>0, pitch/velocity 0..127".into()));
                }
                check_tick_range(n.start, n.duration)?;
            }
            p.track(track)?;
            for n in notes {
                let id = p.alloc();
                let t = p.track_mut(track)?;
                t.notes.push(Note {
                    id,
                    start: n.start,
                    duration: n.duration,
                    pitch: n.pitch,
                    velocity: n.velocity,
                    lyric: n.lyric,
                    expression: None,
                });
                out.note_ids.push(id);
            }
            p.track_mut(track)?
                .notes
                .sort_by_key(|n| (n.start, n.pitch));
        }
        Command::RemoveNotes { track, notes } => {
            let t = p.track_mut(track)?;
            for id in &notes {
                if !t.notes.iter().any(|n| n.id == *id) {
                    return Err(Error::NoteNotFound(*id));
                }
            }
            t.notes.retain(|n| !notes.contains(&n.id));
        }
        Command::MoveNotes {
            track,
            notes,
            delta_tick,
            delta_pitch,
        } => {
            let t = p.track_mut(track)?;
            for id in &notes {
                if !t.notes.iter().any(|n| n.id == *id) {
                    return Err(Error::NoteNotFound(*id));
                }
            }
            for n in t.notes.iter_mut().filter(|n| notes.contains(&n.id)) {
                // keep start + duration inside u32 range
                let max_start = (u32::MAX - n.duration) as i64;
                n.start = (n.start as i64 + delta_tick as i64).clamp(0, max_start) as u32;
                n.pitch = (n.pitch as i16 + delta_pitch as i16).clamp(0, 127) as u8;
            }
            t.notes.sort_by_key(|n| (n.start, n.pitch));
        }
        Command::SetVelocity {
            track,
            notes,
            velocity,
        } => {
            if velocity > 127 {
                return Err(Error::Invalid("velocity 0..127".into()));
            }
            let t = p.track_mut(track)?;
            for n in t.notes.iter_mut().filter(|n| notes.contains(&n.id)) {
                n.velocity = velocity;
            }
        }
        Command::SplitNote { track, note, at } => {
            let new_id = p.alloc();
            let t = p.track_mut(track)?;
            let n = t
                .notes
                .iter_mut()
                .find(|x| x.id == note)
                .ok_or(Error::NoteNotFound(note))?;
            let end = n.start.saturating_add(n.duration);
            if at <= n.start || at >= end {
                return Err(Error::Invalid("split point must be inside the note".into()));
            }
            let tail = Note {
                id: new_id,
                start: at,
                duration: end - at,
                pitch: n.pitch,
                velocity: n.velocity,
                lyric: None,
                expression: n.expression.clone(),
            };
            n.duration = at - n.start;
            t.notes.push(tail);
            t.notes.sort_by_key(|n| (n.start, n.pitch));
            out.note_id = Some(new_id);
        }
        Command::AddAutoPoint {
            track,
            param,
            tick,
            value,
        } => {
            let t = p.track_mut(track)?;
            let (list, v) = match param {
                AutoParam::Gain => (&mut t.gain_auto, value.clamp(0.0, 2.0)),
                AutoParam::Pan => (&mut t.pan_auto, value.clamp(-1.0, 1.0)),
            };
            list.retain(|a| a.tick != tick);
            list.push(AutoPoint { tick, value: v });
            list.sort_by_key(|a| a.tick);
        }
        Command::ClearAuto { track, param } => {
            let t = p.track_mut(track)?;
            match param {
                AutoParam::Gain => t.gain_auto.clear(),
                AutoParam::Pan => t.pan_auto.clear(),
            }
        }
        Command::AddClip {
            track,
            asset,
            start,
            gain,
            offset_ticks,
            duration_ticks,
        } => {
            if !p.assets.iter().any(|a| a.id == asset) {
                return Err(Error::AssetNotFound(asset));
            }
            let id = p.alloc();
            let t = p.track_mut(track)?;
            if t.kind != TrackKind::Audio {
                return Err(Error::Invalid(
                    "clips can only be placed on audio tracks".into(),
                ));
            }
            t.clips.push(AudioClip {
                id,
                asset,
                start,
                gain: gain.unwrap_or(1.0).clamp(0.0, 2.0),
                offset_ticks: offset_ticks.unwrap_or(0),
                duration_ticks,
                fade_in_ticks: 0,
                fade_out_ticks: 0,
            });
            out.clip_id = Some(id);
        }
        Command::RemoveClip { track, clip } => {
            let t = p.track_mut(track)?;
            let n = t.clips.len();
            t.clips.retain(|c| c.id != clip);
            if t.clips.len() == n {
                return Err(Error::ClipNotFound(clip));
            }
        }
        Command::MoveClip { track, clip, start } => {
            let t = p.track_mut(track)?;
            t.clips
                .iter_mut()
                .find(|c| c.id == clip)
                .ok_or(Error::ClipNotFound(clip))?
                .start = start;
        }
        Command::TrimClip {
            track,
            clip,
            start,
            offset_ticks,
            duration_ticks,
        } => {
            let t = p.track_mut(track)?;
            let c = t
                .clips
                .iter_mut()
                .find(|c| c.id == clip)
                .ok_or(Error::ClipNotFound(clip))?;
            if let Some(s) = start {
                c.start = s;
            }
            if let Some(o) = offset_ticks {
                c.offset_ticks = o;
            }
            if let Some(d) = duration_ticks {
                if d == 0 {
                    return Err(Error::Invalid("duration_ticks must be > 0".into()));
                }
                c.duration_ticks = Some(d);
            }
        }
        Command::SplitClip { track, clip, at } => {
            let (asset, gain, offset_ticks, fade_out_ticks, delta, rem) = {
                let clip_len = {
                    let t = p.track(track)?;
                    let c = t
                        .clips
                        .iter()
                        .find(|c| c.id == clip)
                        .ok_or(Error::ClipNotFound(clip))?;
                    p.clip_len_ticks(c)
                };
                let t = p.track_mut(track)?;
                let c = t
                    .clips
                    .iter_mut()
                    .find(|c| c.id == clip)
                    .ok_or(Error::ClipNotFound(clip))?;
                if at <= c.start || at >= c.start.saturating_add(clip_len) {
                    return Err(Error::Invalid(
                        "split point must be strictly inside the clip".into(),
                    ));
                }
                let delta = at - c.start;
                let rem = clip_len - delta;
                let asset = c.asset;
                let gain = c.gain;
                let offset_ticks = c.offset_ticks;
                let fade_out_ticks = c.fade_out_ticks;
                c.duration_ticks = Some(delta);
                c.fade_out_ticks = 0;
                (asset, gain, offset_ticks, fade_out_ticks, delta, rem)
            };
            let new_id = p.alloc();
            let tail = AudioClip {
                id: new_id,
                asset,
                start: at,
                gain,
                offset_ticks: offset_ticks.saturating_add(delta),
                duration_ticks: Some(rem),
                fade_in_ticks: 0,
                fade_out_ticks,
            };
            let t = p.track_mut(track)?;
            t.clips.push(tail);
            t.clips.sort_by_key(|c| c.start);
            out.clip_id = Some(new_id);
        }
        Command::SetClipGain { track, clip, gain } => {
            let t = p.track_mut(track)?;
            let c = t
                .clips
                .iter_mut()
                .find(|c| c.id == clip)
                .ok_or(Error::ClipNotFound(clip))?;
            c.gain = gain.clamp(0.0, 2.0);
        }
        Command::SetClipFade {
            track,
            clip,
            fade_in_ticks,
            fade_out_ticks,
        } => {
            let t = p.track_mut(track)?;
            let c = t
                .clips
                .iter_mut()
                .find(|c| c.id == clip)
                .ok_or(Error::ClipNotFound(clip))?;
            if let Some(fi) = fade_in_ticks {
                c.fade_in_ticks = fi;
            }
            if let Some(fo) = fade_out_ticks {
                c.fade_out_ticks = fo;
            }
        }
        Command::AddFx { track, kind } => {
            let id = p.alloc();
            let inst = FxInstance {
                id,
                enabled: true,
                kind,
            };
            match track {
                Some(t_id) => {
                    let t = p.track_mut(t_id)?;
                    t.fx.push(inst);
                }
                None => {
                    p.master_fx.push(inst);
                }
            }
            out.fx_id = Some(id);
        }
        Command::SetFx {
            track,
            fx,
            kind,
            enabled,
        } => {
            let fx_list = match track {
                Some(t_id) => &mut p.track_mut(t_id)?.fx,
                None => &mut p.master_fx,
            };
            let item = fx_list
                .iter_mut()
                .find(|f| f.id == fx)
                .ok_or(Error::FxNotFound(fx))?;
            if let Some(k) = kind {
                item.kind = k;
            }
            if let Some(en) = enabled {
                item.enabled = en;
            }
        }
        Command::RemoveFx { track, fx } => {
            let fx_list = match track {
                Some(t_id) => &mut p.track_mut(t_id)?.fx,
                None => &mut p.master_fx,
            };
            let len_before = fx_list.len();
            fx_list.retain(|f| f.id != fx);
            if fx_list.len() == len_before {
                return Err(Error::FxNotFound(fx));
            }
        }
        Command::AddSoundfont { name, data } => {
            let id = p.alloc();
            p.soundfonts.push(SoundfontAsset {
                id,
                name,
                data: data.into(),
            });
            out.asset_id = Some(id);
        }
        Command::SetTrackSoundfont { track, soundfont } => {
            if let Some(ref sf) = soundfont {
                if !p.soundfonts.iter().any(|s| s.id == sf.asset) {
                    return Err(Error::SoundfontNotFound(sf.asset));
                }
            }
            let t = p.track_mut(track)?;
            match &mut t.kind {
                TrackKind::Instrument {
                    soundfont: sf_slot, ..
                } => {
                    *sf_slot = soundfont;
                }
                _ => return Err(Error::Invalid("track is not an instrument track".into())),
            }
        }
        Command::SetTrackWaveform { track, waveform } => {
            let t = p.track_mut(track)?;
            match &mut t.kind {
                TrackKind::Instrument { waveform: wf, .. } => *wf = waveform,
                _ => return Err(Error::Invalid("track is not an instrument track".into())),
            }
        }
        Command::SetTrackSynth { track, synth } => {
            let t = p.track_mut(track)?;
            match &mut t.kind {
                TrackKind::Instrument { synth: s_slot, .. } => *s_slot = synth,
                _ => return Err(Error::Invalid("track is not an instrument track".into())),
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
    fn from_json_rejects_invalid_tempo_and_sample_rate() {
        let base = Project::new("x").to_json();
        let with = |k: &str, v: serde_json::Value| {
            let mut j: serde_json::Value = serde_json::from_str(&base).unwrap();
            j[k] = v;
            Project::from_json(&j.to_string())
        };
        assert!(with("tempo", serde_json::json!(0.0)).is_err());
        assert!(with("tempo", serde_json::json!(-50.0)).is_err());
        assert!(with("tempo", serde_json::json!(1000.0)).is_err());
        assert!(with("sample_rate", serde_json::json!(0)).is_err());
        assert!(with("tempo", serde_json::json!(140.0)).is_ok());
        assert!(validate_tempo(f32::NAN).is_err());
    }

    #[test]
    fn tick_arithmetic_never_overflows() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "t".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();
        let add = |e: &mut Engine, start: u32, duration: u32| {
            e.apply(Command::AddNote {
                track: t,
                start,
                duration,
                pitch: 60,
                velocity: 100,
                lyric: None,
            })
        };
        // start + duration > u32::MAX is rejected, not wrapped
        assert!(add(&mut e, u32::MAX - 10, 100).is_err());
        assert!(e.project.tracks[0].notes.is_empty());
        // the largest representable note is accepted and every derived calculation survives it
        let o = add(&mut e, u32::MAX - 100, 100).unwrap();
        assert_eq!(e.project.length_ticks(), u32::MAX);
        e.apply(Command::Quantize {
            track: t,
            grid: 480,
        })
        .unwrap();
        let note = o.note_id.unwrap();
        let (start, dur) = {
            let n = &e.project.tracks[0].notes[0];
            (n.start, n.duration)
        };
        assert!(start.checked_add(dur).is_some());
        e.apply(Command::SplitNote {
            track: t,
            note,
            at: start + 50,
        })
        .unwrap();
        // moving far to the right clamps instead of wrapping
        let ids: Vec<u32> = e.project.tracks[0].notes.iter().map(|n| n.id).collect();
        e.apply(Command::MoveNotes {
            track: t,
            notes: ids,
            delta_tick: i32::MAX,
            delta_pitch: 0,
        })
        .unwrap();
        for n in &e.project.tracks[0].notes {
            assert!(n.start.checked_add(n.duration).is_some());
        }
        // batch and single move enforce the same rule
        assert!(e
            .apply(Command::AddNotes {
                track: t,
                notes: vec![NewNote {
                    start: u32::MAX,
                    duration: 1,
                    pitch: 60,
                    velocity: 100,
                    lyric: None
                }],
            })
            .is_err());
        let id = e.project.tracks[0].notes[0].id;
        assert!(e
            .apply(Command::MoveNote {
                track: t,
                note: id,
                start: Some(u32::MAX),
                pitch: None,
                duration: Some(10),
            })
            .is_err());
    }

    #[test]
    fn from_json_repairs_stale_id_counter() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "a".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        // Simulate a hand-edited / older file whose counter is too low or missing.
        let mut v: serde_json::Value = serde_json::from_str(&e.project.to_json()).unwrap();
        v["next_id"] = serde_json::json!(1);
        let mut p = Project::from_json(&v.to_string()).unwrap();
        assert!(p.alloc() > t);
        v.as_object_mut().unwrap().remove("next_id");
        let mut p = Project::from_json(&v.to_string()).unwrap();
        assert!(p.alloc() > t);
    }

    #[test]
    fn undo_snapshots_share_embedded_audio() {
        let mut e = Engine::default();
        e.import_audio("tone", 44100, &vec![0.25; 44100]);
        e.import_soundfont("sf", b"RIFFfake");
        let t = e
            .apply(Command::AddTrack {
                name: "i".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();
        for i in 0..50 {
            e.apply(Command::AddNote {
                track: t,
                start: i * 10,
                duration: 10,
                pitch: 60,
                velocity: 100,
                lyric: None,
            })
            .unwrap();
        }
        // snapshots taken before the imports hold no assets; every later one must share them
        let with_assets: Vec<_> = e
            .undo
            .iter()
            .filter(|s| !s.assets.is_empty() && !s.soundfonts.is_empty())
            .collect();
        assert!(with_assets.len() >= 50, "{}", with_assets.len());
        for snap in with_assets {
            assert!(Arc::ptr_eq(&snap.assets[0].data, &e.project.assets[0].data));
            assert!(Arc::ptr_eq(
                &snap.soundfonts[0].data,
                &e.project.soundfonts[0].data
            ));
        }
        // payload still round-trips through JSON and decodes
        let back = Project::from_json(&e.project.to_json()).unwrap();
        assert_eq!(back.assets[0].samples().len(), 44100);
        assert_eq!(back.soundfonts[0].bytes(), b"RIFFfake");
        // light JSON drops the payload
        assert!(e.project.to_json_light().len() < e.project.to_json().len());
    }

    #[test]
    fn add_undo_redo() {
        let mut e = Engine::default();
        let o = e
            .apply_json(
                r#"{"cmd":"add_track","name":"Lead","kind":{"type":"vocal","voice":"default"}}"#,
            )
            .unwrap();
        let t = o.track_id.unwrap();
        e.apply(Command::AddNote {
            track: t,
            start: 0,
            duration: 480,
            pitch: 60,
            velocity: 100,
            lyric: Some("la".into()),
        })
        .unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 1);
        e.undo().unwrap();
        assert!(e.project.tracks[0].notes.is_empty());
        e.redo().unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 1);
    }

    #[test]
    fn batch_note_ops_and_split() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "k".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();
        let ids = e
            .apply(Command::AddNotes {
                track: t,
                notes: (0..4)
                    .map(|i| NewNote {
                        start: i * 480,
                        duration: 480,
                        pitch: 60,
                        velocity: 90,
                        lyric: None,
                    })
                    .collect(),
            })
            .unwrap()
            .note_ids;
        assert_eq!(ids.len(), 4);
        e.apply(Command::MoveNotes {
            track: t,
            notes: ids[..2].to_vec(),
            delta_tick: 10,
            delta_pitch: 2,
        })
        .unwrap();
        assert_eq!(
            e.project.tracks[0]
                .notes
                .iter()
                .filter(|n| n.pitch == 62)
                .count(),
            2
        );
        e.apply(Command::SplitNote {
            track: t,
            note: ids[3],
            at: 3 * 480 + 240,
        })
        .unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 5);
        assert!(e
            .apply(Command::SplitNote {
                track: t,
                note: ids[3],
                at: 5
            })
            .is_err());
        e.undo().unwrap();
        assert_eq!(e.project.tracks[0].notes.len(), 4);
    }

    #[test]
    fn automation_eval() {
        let pts = vec![
            AutoPoint {
                tick: 0,
                value: 0.0,
            },
            AutoPoint {
                tick: 480,
                value: 1.0,
            },
        ];
        assert!((eval_auto(&pts, 240.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(eval_auto(&pts, 9999.0, 0.5), 1.0);
        assert_eq!(eval_auto(&[], 10.0, 0.7), 0.7);
    }

    #[test]
    fn audio_asset_clip_and_loop() {
        let mut e = Engine::default();
        let asset = e.import_audio("tone", 44100, &vec![0.5; 44100]);
        let t = e
            .apply(Command::AddTrack {
                name: "a".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        e.apply(Command::AddClip {
            track: t,
            asset,
            start: 480,
            gain: None,
            offset_ticks: None,
            duration_ticks: None,
        })
        .unwrap();
        // 1 second at 120bpm = 2 beats = 960 ticks
        assert_eq!(e.project.length_ticks(), 480 + 960);
        assert!(e.project.to_json_light().len() < e.project.to_json().len());
        assert!(e
            .apply(Command::AddClip {
                track: t,
                asset: 999,
                start: 0,
                gain: None,
                offset_ticks: None,
                duration_ticks: None
            })
            .is_err());
        e.apply(Command::SetLoop {
            start: Some(0),
            end: Some(1920),
        })
        .unwrap();
        assert!(e
            .apply(Command::SetLoop {
                start: Some(5),
                end: Some(2)
            })
            .is_err());
        e.apply(Command::SetTimeSignature {
            numerator: 6,
            denominator: 8,
        })
        .unwrap();
        assert_eq!(e.project.bar_ticks(), 240 * 6);
    }

    #[test]
    fn fx_chain_commands() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "Synth".into(),
                kind: TrackKind::instrument(Waveform::Saw),
            })
            .unwrap()
            .track_id
            .unwrap();

        let o = e
            .apply(Command::AddFx {
                track: Some(t),
                kind: FxKind::Delay {
                    time_ms: 250.0,
                    feedback: 0.4,
                    mix: 0.3,
                    ping_pong: true,
                },
            })
            .unwrap();
        let fx_id = o.fx_id.unwrap();
        assert_eq!(e.project.tracks[0].fx.len(), 1);

        e.apply(Command::SetFx {
            track: Some(t),
            fx: fx_id,
            kind: None,
            enabled: Some(false),
        })
        .unwrap();
        assert!(!e.project.tracks[0].fx[0].enabled);

        e.apply(Command::RemoveFx {
            track: Some(t),
            fx: fx_id,
        })
        .unwrap();
        assert!(e.project.tracks[0].fx.is_empty());
    }

    #[test]
    fn clip_trim_split_fade_and_gain() {
        let mut e = Engine::default();
        let asset = e.import_audio("sample", 44100, &vec![0.8; 44100 * 2]); // 2 seconds = 1920 ticks
        let t = e
            .apply(Command::AddTrack {
                name: "Audio".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        let c = e
            .apply(Command::AddClip {
                track: t,
                asset,
                start: 0,
                gain: Some(0.9),
                offset_ticks: None,
                duration_ticks: None,
            })
            .unwrap()
            .clip_id
            .unwrap();
        assert_eq!(
            e.project.clip_len_ticks(&e.project.tracks[0].clips[0]),
            1920
        );

        // Trim duration to 960
        e.apply(Command::TrimClip {
            track: t,
            clip: c,
            start: None,
            offset_ticks: None,
            duration_ticks: Some(960),
        })
        .unwrap();
        assert_eq!(e.project.clip_len_ticks(&e.project.tracks[0].clips[0]), 960);

        // Split at 480
        let o = e
            .apply(Command::SplitClip {
                track: t,
                clip: c,
                at: 480,
            })
            .unwrap();
        let c2 = o.clip_id.unwrap();
        assert_eq!(e.project.tracks[0].clips.len(), 2);
        assert_eq!(e.project.tracks[0].clips[0].duration_ticks, Some(480));
        assert_eq!(e.project.tracks[0].clips[1].start, 480);
        assert_eq!(e.project.tracks[0].clips[1].offset_ticks, 480);
        assert_eq!(e.project.tracks[0].clips[1].duration_ticks, Some(480));

        // Set gain & fade
        e.apply(Command::SetClipGain {
            track: t,
            clip: c2,
            gain: 1.5,
        })
        .unwrap();
        assert_eq!(e.project.tracks[0].clips[1].gain, 1.5);
        e.apply(Command::SetClipFade {
            track: t,
            clip: c2,
            fade_in_ticks: Some(120),
            fade_out_ticks: Some(60),
        })
        .unwrap();
        assert_eq!(e.project.tracks[0].clips[1].fade_in_ticks, 120);
        assert_eq!(e.project.tracks[0].clips[1].fade_out_ticks, 60);

        // Undo/redo
        e.undo().unwrap(); // undo fade
        assert_eq!(e.project.tracks[0].clips[1].fade_in_ticks, 0);
        e.redo().unwrap();
        assert_eq!(e.project.tracks[0].clips[1].fade_in_ticks, 120);
    }

    #[test]
    fn soundfont_asset_and_commands() {
        let mut e = Engine::default();
        let sf_id = e.import_soundfont("TestSF2", b"RIFFfakeSF2");
        assert_eq!(e.project.soundfonts.len(), 1);
        assert_eq!(e.project.soundfonts[0].name, "TestSF2");

        let t = e
            .apply(Command::AddTrack {
                name: "Piano".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();

        // Assign soundfont preset 0
        e.apply(Command::SetTrackSoundfont {
            track: t,
            soundfont: Some(SoundfontConfig {
                asset: sf_id,
                bank: 0,
                preset: 0,
            }),
        })
        .unwrap();

        match &e.project.tracks[0].kind {
            TrackKind::Instrument {
                soundfont: Some(sf),
                ..
            } => {
                assert_eq!(sf.asset, sf_id);
                assert_eq!(sf.preset, 0);
            }
            _ => panic!("Expected soundfont instrument"),
        }

        // Change waveform
        e.apply(Command::SetTrackWaveform {
            track: t,
            waveform: Waveform::Square,
        })
        .unwrap();

        match &e.project.tracks[0].kind {
            TrackKind::Instrument { waveform, .. } => {
                assert_eq!(*waveform, Waveform::Square);
            }
            _ => panic!("Expected instrument track"),
        }

        // Non-existent soundfont should fail
        assert!(e
            .apply(Command::SetTrackSoundfont {
                track: t,
                soundfont: Some(SoundfontConfig {
                    asset: 9999,
                    bank: 0,
                    preset: 0,
                }),
            })
            .is_err());

        // Test SetTrackSynth
        e.apply(Command::SetTrackSynth {
            track: t,
            synth: Some(SynthConfig {
                filter: SynthFilter::Moog,
                cutoff_hz: 1200.0,
                resonance: 0.65,
                filter_env_amount: 0.5,
                attack: 0.01,
                decay: 0.2,
                sustain: 0.4,
                release: 0.3,
            }),
        })
        .unwrap();

        match &e.project.tracks[0].kind {
            TrackKind::Instrument { synth: Some(s), .. } => {
                assert_eq!(s.filter, SynthFilter::Moog);
                assert_eq!(s.cutoff_hz, 1200.0);
                assert_eq!(s.resonance, 0.65);
            }
            _ => panic!("Expected synth config"),
        }
    }
}
