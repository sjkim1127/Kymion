//! Kymion audio engine: offline renderer, mixer, DSP effects chain, analysis, WAV export.

use kymion_core::{
    eval_auto, ClashSeverity, DiagnosticReport, FrequencyBand, FrequencyClash, FxInstance, FxKind,
    LufsReport, Project, SoundfontAsset, SoundfontConfig, SynthConfig, SynthFilter, Track,
    TrackKind, Waveform, PPQ,
};
use kymion_dsp::{
    midi_to_hz, Adsr, AnalogAdsr, Compressor, Eq3Band, Filterbank6, KWeightingFilter, MoogLadder,
    Osc, StateVariableFilter, StereoDelay, StereoReverb, SvfMode,
};
use kymion_vocal::{render_track as render_vocal, FormantVoice};
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct SoundfontPresetInfo {
    pub bank: u16,
    pub preset: u16,
    pub name: String,
}

/// Recently used parsed SoundFonts. Parsing a multi-megabyte SF2 for every preview note or
/// every track of a render is the dominant cost, so keep a few around. Entries hold the
/// asset's `Arc<str>` payload, which keeps its address unique while cached.
static SF_CACHE: Mutex<Vec<(Arc<str>, Arc<rustysynth::SoundFont>)>> = Mutex::new(Vec::new());
const SF_CACHE_SLOTS: usize = 4;

/// Parse (or fetch from the cache) the SoundFont stored in a project asset.
fn load_soundfont(asset: &SoundfontAsset) -> Option<Arc<rustysynth::SoundFont>> {
    let mut cache = SF_CACHE.lock().ok()?;
    if let Some(pos) = cache.iter().position(|(d, _)| Arc::ptr_eq(d, &asset.data)) {
        let entry = cache.remove(pos);
        let sf = entry.1.clone();
        cache.push(entry); // most recently used goes last
        return Some(sf);
    }
    let bytes = asset.bytes();
    if bytes.is_empty() {
        return None;
    }
    let sf = Arc::new(rustysynth::SoundFont::new(&mut std::io::Cursor::new(bytes)).ok()?);
    if cache.len() >= SF_CACHE_SLOTS {
        cache.remove(0);
    }
    cache.push((asset.data.clone(), sf.clone()));
    Some(sf)
}

pub fn list_soundfont_presets(bytes: &[u8]) -> Vec<SoundfontPresetInfo> {
    let mut cursor = std::io::Cursor::new(bytes);
    let Ok(sf) = rustysynth::SoundFont::new(&mut cursor) else {
        return vec![];
    };
    sf.get_presets()
        .iter()
        .map(|p| SoundfontPresetInfo {
            bank: p.get_bank_number() as u16,
            preset: p.get_patch_number() as u16,
            name: p.get_name().to_string(),
        })
        .collect()
}

/// Level of the mix *before* the master soft limiter, which compresses every
/// sample (|x| < 1 after `tanh`) and would otherwise hide overs from analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LimiterStats {
    /// Largest |sample| seen before limiting.
    pub peak: f32,
    /// Samples whose pre-limiter magnitude reached full scale (>= 1.0).
    pub over_samples: usize,
}

#[derive(Clone, Debug)]
pub struct Stereo {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Pre-limiter statistics; `Some` only for buffers produced by the master render.
    pub limiter: Option<LimiterStats>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub duration_sec: f32,
    pub peak: f32,
    pub rms: f32,
    /// Samples that reached full scale (>= 1.0) *before* the master soft limiter.
    /// For buffers without limiter statistics: samples at >= 0.999 in the buffer.
    pub clipped_samples: usize,
    /// Peak level before the master soft limiter (can exceed 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_limiter_peak: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lufs: Option<LufsReport>,
}

pub fn render_single_note_synth(
    wf: Waveform,
    synth: Option<&SynthConfig>,
    pitch: u8,
    velocity: u8,
    dur: f32,
    sr: u32,
    buf: &mut [f32],
) {
    let srf = sr as f32;
    let hz = midi_to_hz(pitch as f32);
    let vel = velocity as f32 / 127.0;

    if let Some(cfg) = synth {
        let mut osc = Osc::default();
        let mut amp_env = AnalogAdsr::new(cfg.attack, cfg.decay, cfg.sustain, cfg.release, srf);
        let mut flt_env = AnalogAdsr::new(cfg.attack, cfg.decay, cfg.sustain, cfg.release, srf);
        let mut moog = MoogLadder::new();
        let mut svf = StateVariableFilter::new();

        for (i, sample) in buf.iter_mut().enumerate() {
            if i as f32 / srf >= dur {
                amp_env.gate_off();
                flt_env.gate_off();
            }
            let raw = match wf {
                Waveform::Sine => osc.sine(hz, srf),
                Waveform::Saw => osc.saw(hz, srf) * 0.5,
                Waveform::Square => osc.square(hz, srf) * 0.4,
                Waveform::Triangle => osc.triangle(hz, srf),
                Waveform::Pluck => osc.saw(hz, srf) * 0.5,
            };

            let a = amp_env.next();
            let f_mod = flt_env.next();

            let cutoff = if cfg.filter_env_amount != 0.0 {
                let oct = cfg.filter_env_amount * 4.0;
                (cfg.cutoff_hz * 2f32.powf(f_mod * oct)).clamp(20.0, srf * 0.48)
            } else {
                cfg.cutoff_hz.clamp(20.0, srf * 0.48)
            };

            let filtered = match cfg.filter {
                SynthFilter::Moog => moog.process(raw, cutoff, cfg.resonance, srf),
                SynthFilter::Lowpass => {
                    svf.process(raw, cutoff, cfg.resonance, SvfMode::Lowpass, srf)
                }
                SynthFilter::Highpass => {
                    svf.process(raw, cutoff, cfg.resonance, SvfMode::Highpass, srf)
                }
                SynthFilter::Bandpass => {
                    svf.process(raw, cutoff, cfg.resonance, SvfMode::Bandpass, srf)
                }
                SynthFilter::None => raw,
            };

            *sample += filtered * a * vel * 0.5;

            if !amp_env.is_active() && (i as f32 / srf) >= dur {
                break;
            }
        }
    } else {
        let mut osc = Osc::default();
        let mut env = match wf {
            Waveform::Pluck => Adsr::new(0.002, 0.4, 0.0, 0.15, srf),
            _ => Adsr::new(0.01, 0.1, 0.7, 0.25, srf),
        };
        for (i, sample) in buf.iter_mut().enumerate() {
            if i as f32 / srf >= dur {
                env.gate_off();
            }
            let s = match wf {
                Waveform::Sine => osc.sine(hz, srf),
                Waveform::Saw => osc.saw(hz, srf) * 0.5,
                Waveform::Square => osc.square(hz, srf) * 0.4,
                Waveform::Triangle => osc.triangle(hz, srf),
                Waveform::Pluck => osc.saw(hz, srf) * 0.5,
            };
            *sample += s * env.next() * vel * 0.5;
        }
    }
}

fn render_instrument_into(
    p: &Project,
    t: &Track,
    wf: Waveform,
    synth: Option<&SynthConfig>,
    sr: u32,
    buf: &mut [f32],
) {
    let srf = sr as f32;
    for n in &t.notes {
        let dur = p.ticks_to_secs(n.duration);
        let off = (p.ticks_to_secs(n.start) * srf) as usize;
        let release_pad = synth.map(|s| s.release.max(0.1)).unwrap_or(0.35);
        let len = ((dur + release_pad + 0.1) * srf) as usize;
        let note_end = (off + len).min(buf.len());
        if off < buf.len() {
            render_single_note_synth(
                wf,
                synth,
                n.pitch,
                n.velocity,
                dur,
                sr,
                &mut buf[off..note_end],
            );
        }
    }
}

fn render_soundfont_into(
    p: &Project,
    t: &Track,
    sf_cfg: &SoundfontConfig,
    sr: u32,
    out_l: &mut [f32],
    out_r: &mut [f32],
) {
    let Some(sf_asset) = p.soundfonts.iter().find(|sf| sf.id == sf_cfg.asset) else {
        return;
    };
    let Some(soundfont) = load_soundfont(sf_asset) else {
        return;
    };
    let settings = rustysynth::SynthesizerSettings::new(sr as i32);
    let Ok(mut synth) = rustysynth::Synthesizer::new(&soundfont, &settings) else {
        return;
    };

    // Bank and Program change on channel 0
    let bank = sf_cfg.bank as i32;
    synth.process_midi_message(0, 0xB0, 0x00, bank >> 7);
    synth.process_midi_message(0, 0xB0, 0x20, bank & 0x7F);
    synth.process_midi_message(0, 0xC0, sf_cfg.preset as i32, 0);

    let srf = sr as f32;
    let total_samples = out_l.len().min(out_r.len());

    #[derive(Clone, Copy)]
    enum MidiEvt {
        NoteOn { key: i32, vel: i32 },
        NoteOff { key: i32 },
    }

    let mut events: Vec<(usize, MidiEvt)> = Vec::with_capacity(t.notes.len() * 2);
    for n in &t.notes {
        let on = (p.ticks_to_secs(n.start) * srf) as usize;
        let off = (p.ticks_to_secs(n.start.saturating_add(n.duration)) * srf) as usize;
        if on < total_samples {
            events.push((
                on,
                MidiEvt::NoteOn {
                    key: n.pitch as i32,
                    vel: n.velocity as i32,
                },
            ));
        }
        if off < total_samples {
            events.push((
                off,
                MidiEvt::NoteOff {
                    key: n.pitch as i32,
                },
            ));
        }
    }
    events.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| match (&a.1, &b.1) {
            (MidiEvt::NoteOff { .. }, MidiEvt::NoteOn { .. }) => std::cmp::Ordering::Less,
            (MidiEvt::NoteOn { .. }, MidiEvt::NoteOff { .. }) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        })
    });

    let mut cur_sample = 0;
    let mut evt_idx = 0;

    while cur_sample < total_samples {
        let next_sample = if evt_idx < events.len() {
            events[evt_idx].0.min(total_samples)
        } else {
            total_samples
        };

        if next_sample > cur_sample {
            synth.render(
                &mut out_l[cur_sample..next_sample],
                &mut out_r[cur_sample..next_sample],
            );
            cur_sample = next_sample;
        }

        while evt_idx < events.len() && events[evt_idx].0 <= cur_sample {
            match events[evt_idx].1 {
                MidiEvt::NoteOn { key, vel } => synth.note_on(0, key, vel),
                MidiEvt::NoteOff { key } => synth.note_off(0, key),
            }
            evt_idx += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_single_note_soundfont(
    p: &Project,
    sf_cfg: &SoundfontConfig,
    pitch: u8,
    velocity: u8,
    dur: f32,
    sr: u32,
    out_l: &mut [f32],
    out_r: &mut [f32],
) {
    let Some(sf_asset) = p.soundfonts.iter().find(|sf| sf.id == sf_cfg.asset) else {
        return;
    };
    let Some(soundfont) = load_soundfont(sf_asset) else {
        return;
    };
    let settings = rustysynth::SynthesizerSettings::new(sr as i32);
    let Ok(mut synth) = rustysynth::Synthesizer::new(&soundfont, &settings) else {
        return;
    };

    let bank = sf_cfg.bank as i32;
    synth.process_midi_message(0, 0xB0, 0x00, bank >> 7);
    synth.process_midi_message(0, 0xB0, 0x20, bank & 0x7F);
    synth.process_midi_message(0, 0xC0, sf_cfg.preset as i32, 0);

    let srf = sr as f32;
    let off_samples = (dur * srf) as usize;
    let total_samples = out_l.len().min(out_r.len());

    synth.note_on(0, pitch as i32, velocity as i32);

    let mut cur = 0;
    while cur < total_samples {
        let next = if cur < off_samples {
            off_samples.min(total_samples)
        } else {
            total_samples
        };

        if next > cur {
            synth.render(&mut out_l[cur..next], &mut out_r[cur..next]);
            cur = next;
        }

        if cur == off_samples {
            synth.note_off(0, pitch as i32);
            synth.render(
                &mut out_l[cur..total_samples],
                &mut out_r[cur..total_samples],
            );
            break;
        }
    }
}

/// Render a single note audition preview for an interactive track.
pub fn render_note_preview(
    p: &Project,
    track_id: u32,
    pitch: u8,
    duration_secs: f32,
    velocity: u8,
) -> Stereo {
    let sr = p.sample_rate;
    let srf = sr as f32;
    let dur = duration_secs.clamp(0.05, 2.0);
    let total_samples = ((dur + 0.35) * srf) as usize;

    let mut left = vec![0.0f32; total_samples];
    let mut right = vec![0.0f32; total_samples];

    let Some(t) = p.tracks.iter().find(|t| t.id == track_id) else {
        return Stereo {
            sample_rate: sr,
            left,
            right,
            limiter: None,
        };
    };

    match &t.kind {
        TrackKind::Instrument {
            waveform,
            soundfont,
            synth,
        } => {
            if let Some(sf_cfg) = soundfont {
                render_single_note_soundfont(
                    p, sf_cfg, pitch, velocity, dur, sr, &mut left, &mut right,
                );
            } else {
                let mut mono = vec![0.0f32; total_samples];
                render_single_note_synth(
                    *waveform,
                    synth.as_ref(),
                    pitch,
                    velocity,
                    dur,
                    sr,
                    &mut mono,
                );
                left.copy_from_slice(&mono);
                right.copy_from_slice(&mono);
            }
        }
        TrackKind::Vocal { .. } => {
            let mut dummy_track = t.clone();
            dummy_track.notes = vec![kymion_core::Note {
                id: 1,
                start: 0,
                duration: p.secs_to_ticks(dur),
                pitch,
                velocity,
                lyric: Some("아".to_string()),
                expression: None,
            }];
            let v = render_vocal(p, &dummy_track, &FormantVoice, sr);
            let len = v.len().min(total_samples);
            left[..len].copy_from_slice(&v[..len]);
            right[..len].copy_from_slice(&v[..len]);
        }
        TrackKind::Audio => {}
    }

    // Apply track's insert FX chain
    apply_fx_chain(&mut left, &mut right, &t.fx, sr);

    Stereo {
        sample_rate: sr,
        left,
        right,
        limiter: None,
    }
}

/// Apply an FX chain to in-place stereo buffers.
pub fn apply_fx_chain(left: &mut [f32], right: &mut [f32], fx_list: &[FxInstance], sr: u32) {
    let srf = sr as f32;
    for fx in fx_list {
        if !fx.enabled {
            continue;
        }
        match &fx.kind {
            FxKind::Eq {
                low_gain,
                mid_gain,
                mid_freq,
                mid_q,
                high_gain,
            } => {
                let mut eq_l =
                    Eq3Band::new(*low_gain, *mid_gain, *mid_freq, *mid_q, *high_gain, srf);
                let mut eq_r =
                    Eq3Band::new(*low_gain, *mid_gain, *mid_freq, *mid_q, *high_gain, srf);
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    *l = eq_l.process(*l);
                    *r = eq_r.process(*r);
                }
            }
            FxKind::Delay {
                time_ms,
                feedback,
                mix,
                ping_pong,
            } => {
                let mut delay = StereoDelay::new(*time_ms, *feedback, *mix, *ping_pong, srf);
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    let (out_l, out_r) = delay.process(*l, *r);
                    *l = out_l;
                    *r = out_r;
                }
            }
            FxKind::Reverb {
                room_size,
                damping,
                width,
                mix,
            } => {
                let mut reverb = StereoReverb::new(*room_size, *damping, *width, *mix, srf);
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    let (out_l, out_r) = reverb.process(*l, *r);
                    *l = out_l;
                    *r = out_r;
                }
            }
            FxKind::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => {
                let mut comp = Compressor::new(
                    *threshold_db,
                    *ratio,
                    *attack_ms,
                    *release_ms,
                    *makeup_db,
                    srf,
                );
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    let (out_l, out_r) = comp.process(*l, *r);
                    *l = out_l;
                    *r = out_r;
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Mix a click track (accented downbeat) into the master output.
    pub metronome: bool,
}

/// Mono audio track: place every clip's asset (linear-resampled) on the timeline,
/// respecting clip start, asset offset (start trim), duration (end trim), gain, and fades.
fn render_audio_track_into(p: &Project, t: &Track, sr: u32, buf: &mut [f32]) {
    for clip in &t.clips {
        let Some(asset) = p.assets.iter().find(|a| a.id == clip.asset) else {
            continue;
        };
        let data = asset.samples();
        if data.is_empty() {
            continue;
        }
        let asset_sr = asset.sample_rate as f32;
        let ratio = asset_sr / sr as f32;
        let asset_offset_samples = (p.ticks_to_secs(clip.offset_ticks) * asset_sr) as usize;
        let clip_len_ticks = p.clip_len_ticks(clip);
        let clip_len_sec = p.ticks_to_secs(clip_len_ticks);
        let out_len = (clip_len_sec * sr as f32) as usize;
        let off = (p.ticks_to_secs(clip.start) * sr as f32) as usize;

        let fade_in_samples = (p.ticks_to_secs(clip.fade_in_ticks) * sr as f32) as usize;
        let fade_out_samples = (p.ticks_to_secs(clip.fade_out_ticks) * sr as f32) as usize;

        let max_samples = out_len.min(buf.len().saturating_sub(off));

        for i in 0..max_samples {
            let pos = asset_offset_samples as f32 + i as f32 * ratio;
            if pos >= data.len() as f32 {
                break;
            }
            let k = pos as usize;
            let frac = pos - k as f32;
            let a = data[k.min(data.len() - 1)];
            let b = data[(k + 1).min(data.len() - 1)];
            let raw_sample = a + (b - a) * frac;

            let mut fade = 1.0f32;
            if fade_in_samples > 0 && i < fade_in_samples {
                fade = fade.min(i as f32 / fade_in_samples as f32);
            }
            if fade_out_samples > 0 && i >= out_len.saturating_sub(fade_out_samples) {
                let rem = out_len - i;
                fade = fade.min(rem as f32 / fade_out_samples as f32);
            }

            buf[off + i] += raw_sample * clip.gain * fade;
        }
    }
}

fn add_metronome(p: &Project, l: &mut [f32], r: &mut [f32], sr: u32) {
    let srf = sr as f32;
    let beat = p.beat_ticks();
    let blip = (0.04 * srf) as usize;
    // length_ticks() decodes every audio asset: compute it once, not once per beat
    let last_tick = p.length_ticks().saturating_add(p.bar_ticks());
    let mut b = 0u32;
    loop {
        let tick = b.saturating_mul(beat);
        let start = (p.ticks_to_secs(tick) * srf) as usize;
        if start >= l.len() || tick > last_tick {
            break;
        }
        let down = b.is_multiple_of(p.time_sig_num.max(1) as u32);
        let hz = if down { 1500.0 } else { 1000.0 };
        for i in 0..blip {
            let env = (-(i as f32) / (0.01 * srf)).exp();
            let v = (std::f32::consts::TAU * hz * i as f32 / srf).sin() * env * 0.3;
            if let (Some(a), Some(c)) = (l.get_mut(start + i), r.get_mut(start + i)) {
                *a += v;
                *c += v;
            }
        }
        b += 1;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Meter {
    pub peak: f32,
    pub rms: f32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StereoMeter {
    pub left: Meter,
    pub right: Meter,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProjectMeters {
    pub master: StereoMeter,
    pub tracks: std::collections::BTreeMap<u32, Meter>,
}

pub const METER_BLOCK_SIZE: usize = 256;

#[derive(Clone, Debug)]
pub struct TrackMeterEnvelope {
    pub id: u32,
    pub peaks: Vec<f32>,
    pub rms: Vec<f32>,
}

impl TrackMeterEnvelope {
    pub fn from_audio(id: u32, left: &[f32], right: &[f32]) -> Self {
        let n = left.len().min(right.len());
        let num_blocks = n.div_ceil(METER_BLOCK_SIZE);
        let mut peaks = Vec::with_capacity(num_blocks);
        let mut rms = Vec::with_capacity(num_blocks);

        for blk in 0..num_blocks {
            let start = blk * METER_BLOCK_SIZE;
            let end = (start + METER_BLOCK_SIZE).min(n);
            let mut p = 0.0f32;
            let mut sum_sq = 0.0f64;
            let count = end - start;
            for i in start..end {
                let l = left[i].abs();
                let r = right[i].abs();
                p = p.max(l).max(r);
                sum_sq += (l as f64) * (l as f64) + (r as f64) * (r as f64);
            }
            peaks.push(p);
            rms.push((sum_sq / (2 * count).max(1) as f64).sqrt() as f32);
        }

        Self { id, peaks, rms }
    }

    #[inline]
    pub fn meter_at(&self, start: usize, len: usize) -> Meter {
        let start_blk = start / METER_BLOCK_SIZE;
        let end_blk = (start + len)
            .div_ceil(METER_BLOCK_SIZE)
            .min(self.peaks.len());
        if start_blk >= self.peaks.len() || start_blk >= end_blk {
            return Meter::default();
        }
        let mut p = 0.0f32;
        let mut sum_sq = 0.0f64;
        let count = end_blk - start_blk;
        for i in start_blk..end_blk {
            p = p.max(self.peaks[i]);
            sum_sq += (self.rms[i] as f64) * (self.rms[i] as f64);
        }
        Meter {
            peak: p,
            rms: (sum_sq / count as f64).sqrt() as f32,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackBandEnergy {
    pub id: u32,
    pub name: String,
    pub band_rms: Vec<[f32; 6]>,
}

impl TrackBandEnergy {
    pub fn from_audio(id: u32, name: &str, left: &[f32], right: &[f32], sr: f32) -> Self {
        let n = left.len().min(right.len());
        let frame_samples = (0.100 * sr).max(10.0) as usize;
        let num_frames = n / frame_samples;
        let mut band_rms = Vec::with_capacity(num_frames);

        let mut fb = Filterbank6::new(sr);

        for f in 0..num_frames {
            let start = f * frame_samples;
            let end = start + frame_samples;
            let mut sum_sq = [0.0f32; 6];
            for i in start..end {
                let mono = (left[i] + right[i]) * 0.5;
                let bands = fb.process(mono);
                for b in 0..6 {
                    sum_sq[b] += bands[b] * bands[b];
                }
            }
            let mut rms = [0.0f32; 6];
            for b in 0..6 {
                rms[b] = (sum_sq[b] / frame_samples as f32).sqrt();
            }
            band_rms.push(rms);
        }

        Self {
            id,
            name: name.to_string(),
            band_rms,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FullRender {
    pub master: Stereo,
    pub track_meters: Vec<TrackMeterEnvelope>,
    pub track_energies: Vec<TrackBandEnergy>,
}

pub fn calc_meter_channel(samples: &[f32], start: usize, len: usize) -> Meter {
    if samples.is_empty() || len == 0 || start >= samples.len() {
        return Meter::default();
    }
    let end = (start + len).min(samples.len());
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let count = end - start;
    for &s in &samples[start..end] {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += (s as f64) * (s as f64);
    }
    Meter {
        peak,
        rms: (sum_sq / count.max(1) as f64).sqrt() as f32,
    }
}

impl FullRender {
    pub fn meters_at(&self, start: usize, len: usize) -> ProjectMeters {
        let ml = calc_meter_channel(&self.master.left, start, len);
        let mr = calc_meter_channel(&self.master.right, start, len);
        let mut tracks = std::collections::BTreeMap::new();
        for t in &self.track_meters {
            tracks.insert(t.id, t.meter_at(start, len));
        }
        ProjectMeters {
            master: StereoMeter {
                left: ml,
                right: mr,
            },
            tracks,
        }
    }
}

/// Constant-power pan law: `pan` in -1..1 -> (left, right) gains; centre is -3 dB per side.
fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (a.cos(), a.sin())
}

/// Render the whole project to stereo including track and master FX chains.
pub fn render(p: &Project) -> Stereo {
    render_with(p, &RenderOptions::default())
}

pub fn render_with(p: &Project, opts: &RenderOptions) -> Stereo {
    render_full(p, opts).master
}

pub fn render_full(p: &Project, opts: &RenderOptions) -> FullRender {
    let sr = p.sample_rate;
    let srf = sr as f32;
    // Tail room for delays/reverbs (extra 2 seconds)
    let total_samples = (p.ticks_to_secs(p.length_ticks()) * srf) as usize + sr as usize * 2;

    let mut master_l: Vec<f32> = vec![0.0; total_samples];
    let mut master_r: Vec<f32> = vec![0.0; total_samples];

    // Reusable scratch buffers to eliminate per-track allocations
    let mut mono: Vec<f32> = vec![0.0; total_samples];
    let mut trk_l: Vec<f32> = vec![0.0; total_samples];
    let mut trk_r: Vec<f32> = vec![0.0; total_samples];

    let mut track_meters: Vec<TrackMeterEnvelope> = Vec::new();
    let mut track_energies: Vec<TrackBandEnergy> = Vec::new();
    let any_solo = p.tracks.iter().any(|t| t.solo);
    let ticks_per_sample = p.tempo * PPQ as f32 / 60.0 / srf;

    for t in p.tracks.iter().filter(|t| !t.mute && (!any_solo || t.solo)) {
        mono.fill(0.0);
        trk_l.fill(0.0);
        trk_r.fill(0.0);
        let mut is_stereo = false;

        match &t.kind {
            TrackKind::Instrument {
                waveform,
                soundfont,
                synth,
            } => {
                if let Some(sf_cfg) = soundfont {
                    render_soundfont_into(p, t, sf_cfg, sr, &mut trk_l, &mut trk_r);
                    is_stereo = true;
                } else {
                    render_instrument_into(p, t, *waveform, synth.as_ref(), sr, &mut mono);
                }
            }
            TrackKind::Vocal { .. } => {
                let v = render_vocal(p, t, &FormantVoice, sr);
                let copy_len = v.len().min(total_samples);
                mono[..copy_len].copy_from_slice(&v[..copy_len]);
            }
            TrackKind::Audio => render_audio_track_into(p, t, sr, &mut mono),
        };

        let static_pan = pan_gains(t.pan);
        let automated = !t.gain_auto.is_empty() || !t.pan_auto.is_empty();
        // Same gain + constant-power pan law for every track kind (mono sources and
        // already-stereo SoundFont output), so equal settings sound equally loud.
        let gain_pan_at = |i: usize| -> (f32, (f32, f32)) {
            if automated {
                let tick = i as f32 * ticks_per_sample;
                (
                    eval_auto(&t.gain_auto, tick, t.gain),
                    pan_gains(eval_auto(&t.pan_auto, tick, t.pan)),
                )
            } else {
                (t.gain, static_pan)
            }
        };

        if is_stereo {
            for i in 0..total_samples {
                let (gain, (pl, pr)) = gain_pan_at(i);
                trk_l[i] *= gain * pl;
                trk_r[i] *= gain * pr;
            }
        } else {
            for i in 0..total_samples {
                let s = mono[i];
                let (gain, (pl, pr)) = gain_pan_at(i);
                trk_l[i] = s * gain * pl;
                trk_r[i] = s * gain * pr;
            }
        }

        // Apply track FX insert chain
        apply_fx_chain(&mut trk_l, &mut trk_r, &t.fx, sr);

        for i in 0..total_samples {
            master_l[i] += trk_l[i];
            master_r[i] += trk_r[i];
        }

        track_meters.push(TrackMeterEnvelope::from_audio(t.id, &trk_l, &trk_r));
        track_energies.push(TrackBandEnergy::from_audio(
            t.id, &t.name, &trk_l, &trk_r, srf,
        ));
    }

    // Apply master bus FX chain
    apply_fx_chain(&mut master_l, &mut master_r, &p.master_fx, sr);

    if opts.metronome {
        add_metronome(p, &mut master_l, &mut master_r, sr);
    }

    // Soft limiter on master output (record pre-limiter level so overs stay visible)
    let mut limiter = LimiterStats::default();
    for s in master_l.iter_mut().chain(master_r.iter_mut()) {
        let a = s.abs();
        if a > limiter.peak {
            limiter.peak = a;
        }
        if a >= 1.0 {
            limiter.over_samples += 1;
        }
        *s = s.tanh();
    }

    FullRender {
        master: Stereo {
            sample_rate: sr,
            left: master_l,
            right: master_r,
            limiter: Some(limiter),
        },
        track_meters,
        track_energies,
    }
}

/// Decode WAV bytes (8/16/24/32-bit int or 32-bit float) to mono f32 + sample rate.
pub fn decode_wav(bytes: &[u8]) -> Result<(u32, Vec<f32>), String> {
    let mut reader =
        hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|v| v as f32 / max)
                .collect()
        }
    };
    let mono = raw
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok((spec.sample_rate, mono))
}

/// Minimum sustained overlap: 1.0 second (10 frames @ 100 ms).
const MIN_CLASH_FRAMES: usize = 10;

/// Overlap score (0..1) if tracks `a` and `b` clash in `band_idx` during frame `f`.
fn frame_clash_overlap(
    a: &TrackBandEnergy,
    b: &TrackBandEnergy,
    f: usize,
    band_idx: usize,
) -> Option<f32> {
    let ea = a.band_rms[f][band_idx];
    let eb = b.band_rms[f][band_idx];
    let tot_a: f32 = a.band_rms[f].iter().sum();
    let tot_b: f32 = b.band_rms[f].iter().sum();
    if tot_a <= 0.005 || tot_b <= 0.005 {
        return None;
    }
    let share_a = ea / tot_a;
    let share_b = eb / tot_b;

    // Clash occurs if both tracks concentrate significant energy in this band (>18%),
    // or if their absolute levels in this band are comparable and audible.
    let is_spectral_clash = share_a > 0.18 && share_b > 0.18 && ea > 0.002 && eb > 0.002;
    let is_level_clash = ea > 0.005 && eb > 0.005 && (ea.min(eb) / ea.max(eb)) > 0.35;
    if is_spectral_clash || is_level_clash {
        Some((share_a.min(share_b) / share_a.max(share_b)).max(ea.min(eb) / ea.max(eb)))
    } else {
        None
    }
}

pub fn detect_frequency_clashes(energies: &[TrackBandEnergy]) -> Vec<FrequencyClash> {
    let mut clashes = Vec::new();
    let num_tracks = energies.len();
    if num_tracks < 2 {
        return clashes;
    }

    for i in 0..num_tracks {
        for j in (i + 1)..num_tracks {
            let a = &energies[i];
            let b = &energies[j];
            let num_frames = a.band_rms.len().min(b.band_rms.len());
            if num_frames == 0 {
                continue;
            }

            for band_idx in 0..6 {
                // Only count *contiguous* runs of at least MIN_CLASH_FRAMES frames, so scattered
                // single frames over a long song do not add up to a "sustained" clash.
                let mut clash_frames = 0usize;
                let mut sum_overlap = 0.0f32;
                let (mut run_frames, mut run_overlap) = (0usize, 0.0f32);
                for f in 0..=num_frames {
                    // `f == num_frames` is a sentinel that flushes the final run
                    let hit = if f < num_frames {
                        frame_clash_overlap(a, b, f, band_idx)
                    } else {
                        None
                    };
                    match hit {
                        Some(o) => {
                            run_frames += 1;
                            run_overlap += o;
                        }
                        None => {
                            if run_frames >= MIN_CLASH_FRAMES {
                                clash_frames += run_frames;
                                sum_overlap += run_overlap;
                            }
                            run_frames = 0;
                            run_overlap = 0.0;
                        }
                    }
                }

                if clash_frames >= MIN_CLASH_FRAMES {
                    let avg_overlap = sum_overlap / clash_frames as f32;
                    let duration_sec = clash_frames as f32 * 0.1;
                    let band = FrequencyBand::ALL[band_idx];
                    let (f_low, f_high) = band.freq_range_hz();

                    let severity = if avg_overlap > 0.75 && duration_sec >= 3.0 {
                        ClashSeverity::High
                    } else if avg_overlap > 0.60 || duration_sec >= 4.0 {
                        ClashSeverity::Medium
                    } else {
                        ClashSeverity::Low
                    };

                    let suggestion = match band {
                        FrequencyBand::SubBass | FrequencyBand::Bass => {
                            format!(
                                "Tracks '{}' and '{}' compete in the {} ({:.0}-{:.0} Hz) for {:.1}s. Apply sidechain ducking on '{}' triggered by '{}', or high-pass filter.",
                                a.name, b.name, band.label(), f_low, f_high, duration_sec, b.name, a.name
                            )
                        }
                        FrequencyBand::LowMid => {
                            format!(
                                "Muddiness detected between '{}' and '{}' in {} ({:.0}-{:.0} Hz) for {:.1}s. Carve out 250-400 Hz by 2~3 dB with EQ on the background track.",
                                a.name, b.name, band.label(), f_low, f_high, duration_sec
                            )
                        }
                        FrequencyBand::Mid | FrequencyBand::HighMid => {
                            format!(
                                "Masking clash between '{}' and '{}' in {} ({:.0}-{:.0} Hz) for {:.1}s. Pan tracks apart or carve out competing frequencies with EQ notch on '{}'.",
                                a.name, b.name, band.label(), f_low, f_high, duration_sec, b.name
                            )
                        }
                        FrequencyBand::High => {
                            format!(
                                "High-frequency buildup between '{}' and '{}' in {} ({:.0}-{:.0} Hz) for {:.1}s. Consider high-shelf attenuation or gentle de-essing.",
                                a.name, b.name, band.label(), f_low, f_high, duration_sec
                            )
                        }
                    };

                    clashes.push(FrequencyClash {
                        track_a_id: a.id,
                        track_a_name: a.name.clone(),
                        track_b_id: b.id,
                        track_b_name: b.name.clone(),
                        band,
                        freq_range_hz: (f_low, f_high),
                        severity,
                        overlap_score: (avg_overlap * 100.0).round() / 100.0,
                        clash_duration_sec: (duration_sec * 10.0).round() / 10.0,
                        suggestion,
                    });
                }
            }
        }
    }

    clashes.sort_by(|x, y| {
        let score = |s: ClashSeverity| match s {
            ClashSeverity::High => 3,
            ClashSeverity::Medium => 2,
            ClashSeverity::Low => 1,
        };
        score(y.severity).cmp(&score(x.severity)).then_with(|| {
            y.overlap_score
                .partial_cmp(&x.overlap_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });

    clashes
}

/// Taps per polyphase branch of the true-peak interpolator (BS.1770-4 uses 12).
const TP_TAPS: usize = 12;
/// Samples before the interpolation point that the first tap reads.
const TP_PRE: usize = TP_TAPS / 2 - 1;

/// Polyphase windowed-sinc kernels for the three in-between positions (1/4, 2/4,
/// 3/4) of a 4x oversampler. Each branch is normalised to unity DC gain.
fn true_peak_kernel() -> [[f32; TP_TAPS]; 3] {
    let half = (TP_TAPS / 2) as f64;
    let mut k = [[0.0f32; TP_TAPS]; 3];
    for (phase, row) in k.iter_mut().enumerate() {
        let frac = (phase + 1) as f64 / 4.0;
        let mut taps = [0.0f64; TP_TAPS];
        for (j, t) in taps.iter_mut().enumerate() {
            // distance (in samples) between the interpolation point and tap j
            let d = frac - (j as f64 - TP_PRE as f64);
            let x = std::f64::consts::PI * d;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * d / half).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * d / half).cos();
            *t = sinc * w;
        }
        let sum: f64 = taps.iter().sum();
        for (o, t) in row.iter_mut().zip(taps) {
            *o = (t / sum) as f32;
        }
    }
    k
}

/// Largest |value| of the interpolated points between the samples of `x`.
fn oversampled_peak(x: &[f32], kernel: &[[f32; TP_TAPS]; 3]) -> f32 {
    let len = x.len();
    let mut peak = 0.0f32;
    let at = |i: isize| -> f32 {
        if i >= 0 && (i as usize) < len {
            x[i as usize]
        } else {
            0.0
        }
    };
    for n in 0..len.saturating_sub(1) {
        let interior = n >= TP_PRE && n + TP_TAPS - TP_PRE <= len;
        for row in kernel {
            let acc: f32 = if interior {
                let w = &x[n - TP_PRE..n - TP_PRE + TP_TAPS];
                row.iter().zip(w).map(|(h, v)| h * v).sum()
            } else {
                row.iter()
                    .enumerate()
                    .map(|(j, h)| h * at(n as isize + j as isize - TP_PRE as isize))
                    .sum()
            };
            peak = peak.max(acc.abs());
        }
    }
    peak
}

/// Calculate ITU-R BS.1770-4 gated integrated loudness (LUFS), momentary max, short-term max, LRA, and True Peak.
pub fn calc_lufs(s: &Stereo) -> LufsReport {
    let srf = s.sample_rate as f32;
    let n = s.left.len().min(s.right.len());
    if n == 0 {
        return LufsReport::default();
    }

    let mut k_l = KWeightingFilter::new(srf);
    let mut k_r = KWeightingFilter::new(srf);
    let mut filtered_l = Vec::with_capacity(n);
    let mut filtered_r = Vec::with_capacity(n);

    let mut true_peak = 0.0f32;
    for i in 0..n {
        let sl = s.left[i];
        let sr_samp = s.right[i];
        true_peak = true_peak.max(sl.abs()).max(sr_samp.abs());
        filtered_l.push(k_l.process(sl));
        filtered_r.push(k_r.process(sr_samp));
    }
    // Inter-sample peaks via 4x oversampling (BS.1770-4 Annex 2 style).
    let kernel = true_peak_kernel();
    true_peak = true_peak
        .max(oversampled_peak(&s.left[..n], &kernel))
        .max(oversampled_peak(&s.right[..n], &kernel));

    let true_peak_db = if true_peak > 1e-6 {
        20.0 * true_peak.log10()
    } else {
        -120.0
    };

    let block_size = (0.400 * srf).max(16.0) as usize;
    let hop_size = (0.100 * srf).max(4.0) as usize;

    if n < block_size {
        let mut sum_sq = 0.0f64;
        for i in 0..n {
            sum_sq += (filtered_l[i] as f64).powi(2) + (filtered_r[i] as f64).powi(2);
        }
        let mean_sq = sum_sq / (2 * n) as f64;
        let lkfs = if mean_sq > 1e-12 {
            -0.691 + 10.0 * mean_sq.log10() as f32
        } else {
            -70.0
        };
        return LufsReport {
            integrated: lkfs,
            momentary_max: lkfs,
            short_term_max: lkfs,
            loudness_range: 0.0,
            true_peak_db,
        };
    }

    let mut z_blocks: Vec<f64> = Vec::new();
    let mut momentary_max = -120.0f32;

    let mut start = 0;
    while start + block_size <= n {
        let mut sum_sq = 0.0f64;
        for i in start..start + block_size {
            sum_sq += (filtered_l[i] as f64).powi(2) + (filtered_r[i] as f64).powi(2);
        }
        let z = sum_sq / (2 * block_size) as f64;
        z_blocks.push(z);

        let m_loudness = if z > 1e-12 {
            -0.691 + 10.0 * z.log10() as f32
        } else {
            -70.0
        };
        momentary_max = momentary_max.max(m_loudness);

        start += hop_size;
    }

    let st_blocks = 30.min(z_blocks.len());
    let mut short_term_max = -120.0f32;
    let mut st_values: Vec<f32> = Vec::new();

    if z_blocks.len() >= st_blocks {
        let mut window_sum: f64 = z_blocks[..st_blocks].iter().sum();
        let val = -0.691 + 10.0 * (window_sum / st_blocks as f64).max(1e-12).log10() as f32;
        short_term_max = short_term_max.max(val);
        st_values.push(val);

        for i in st_blocks..z_blocks.len() {
            window_sum += z_blocks[i] - z_blocks[i - st_blocks];
            let val = -0.691 + 10.0 * (window_sum / st_blocks as f64).max(1e-12).log10() as f32;
            short_term_max = short_term_max.max(val);
            st_values.push(val);
        }
    } else {
        short_term_max = momentary_max;
    }

    // Two-stage gating for integrated loudness per BS.1770-4
    let mut abs_gated: Vec<f64> = Vec::new();
    for &z in &z_blocks {
        let l = -0.691 + 10.0 * z.max(1e-12).log10() as f32;
        if l > -70.0 {
            abs_gated.push(z);
        }
    }

    if abs_gated.is_empty() {
        return LufsReport {
            integrated: -70.0,
            momentary_max: momentary_max.max(-70.0),
            short_term_max: short_term_max.max(-70.0),
            loudness_range: 0.0,
            true_peak_db,
        };
    }

    let mean_z_abs: f64 = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
    let l_abs = -0.691 + 10.0 * mean_z_abs.max(1e-12).log10() as f32;

    let gamma_rel = l_abs - 10.0;
    let mut rel_gated: Vec<f64> = Vec::new();
    for &z in &abs_gated {
        let l = -0.691 + 10.0 * z.max(1e-12).log10() as f32;
        if l > gamma_rel {
            rel_gated.push(z);
        }
    }

    let integrated = if !rel_gated.is_empty() {
        let mean_z_rel = rel_gated.iter().sum::<f64>() / rel_gated.len() as f64;
        -0.691 + 10.0 * mean_z_rel.max(1e-12).log10() as f32
    } else {
        l_abs
    };

    // Loudness Range (LRA) per EBU R128
    let mut valid_st: Vec<f32> = st_values
        .into_iter()
        .filter(|&v| v > -70.0 && v > (integrated - 20.0))
        .collect();
    valid_st.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let loudness_range = if valid_st.len() >= 10 {
        let idx_10 = (valid_st.len() as f32 * 0.10) as usize;
        let idx_95 = (valid_st.len() as f32 * 0.95).min((valid_st.len() - 1) as f32) as usize;
        (valid_st[idx_95] - valid_st[idx_10]).max(0.0)
    } else {
        0.0
    };

    LufsReport {
        integrated: (integrated * 10.0).round() / 10.0,
        momentary_max: (momentary_max * 10.0).round() / 10.0,
        short_term_max: (short_term_max * 10.0).round() / 10.0,
        loudness_range: (loudness_range * 10.0).round() / 10.0,
        true_peak_db: (true_peak_db * 10.0).round() / 10.0,
    }
}

pub fn analyze(s: &Stereo) -> Analysis {
    let n = s.left.len().max(1);
    let (mut peak, mut sum, mut clip) = (0f32, 0f64, 0usize);
    for x in s.left.iter().chain(s.right.iter()) {
        peak = peak.max(x.abs());
        sum += (*x as f64).powi(2);
        if x.abs() >= 0.999 {
            clip += 1;
        }
    }
    let lufs = Some(calc_lufs(s));
    let (clipped_samples, pre_limiter_peak) = match s.limiter {
        Some(l) => (l.over_samples, Some(l.peak)),
        None => (clip, None),
    };
    Analysis {
        duration_sec: n as f32 / s.sample_rate as f32,
        peak,
        rms: (sum / (2 * n) as f64).sqrt() as f32,
        clipped_samples,
        pre_limiter_peak,
        lufs,
    }
}

/// Comprehensive mixing diagnostics for AI agents and audio engineers.
pub fn diagnose(p: &Project) -> DiagnosticReport {
    let full = render_full(p, &RenderOptions::default());
    let lufs = calc_lufs(&full.master);
    let clashes = detect_frequency_clashes(&full.track_energies);

    let mut summary = format!(
        "Loudness: {:.1} LUFS (Integrated), Peak: {:.1} dBTP, LRA: {:.1} LU.",
        lufs.integrated, lufs.true_peak_db, lufs.loudness_range
    );

    if lufs.integrated > -12.0 {
        summary.push_str(
            " Warning: Master is loud (> -12 LUFS); streaming services will attenuate it.",
        );
    } else if lufs.integrated < -18.0 {
        summary.push_str(
            " Note: Master is quiet (< -18 LUFS); consider gentle compression or makeup gain.",
        );
    } else {
        summary.push_str(" Optimal loudness for streaming platforms (-14 ± 2 LUFS).");
    }

    if clashes.is_empty() {
        summary.push_str(" Zero frequency masking clashes detected across all tracks.");
    } else {
        summary.push_str(&format!(
            " Found {} frequency masking clash(es) requiring EQ or sidechain adjustments.",
            clashes.len()
        ));
    }

    DiagnosticReport {
        lufs,
        clashes,
        summary,
    }
}

/// 16-bit PCM stereo WAV bytes.
pub fn to_wav_bytes(s: &Stereo) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: s.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
        for (a, b) in s.left.iter().zip(&s.right) {
            w.write_sample((a.clamp(-1.0, 1.0) * 32767.0) as i16)
                .unwrap();
            w.write_sample((b.clamp(-1.0, 1.0) * 32767.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
    }
    cur.into_inner()
}

/// Interleaved f32 (for WebAudio / wasm).
pub fn interleave(s: &Stereo) -> Vec<f32> {
    s.left
        .iter()
        .zip(&s.right)
        .flat_map(|(a, b)| [*a, *b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kymion_core::*;

    #[test]
    fn renders_with_fx() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "v".into(),
                kind: TrackKind::Vocal {
                    voice: "default".into(),
                },
            })
            .unwrap()
            .track_id
            .unwrap();

        e.apply(Command::AddNote {
            track: t,
            start: 0,
            duration: 480,
            pitch: 64,
            velocity: 100,
            lyric: Some("라".into()),
        })
        .unwrap();

        e.apply(Command::AddFx {
            track: Some(t),
            kind: FxKind::Reverb {
                room_size: 0.8,
                damping: 0.2,
                width: 1.0,
                mix: 0.5,
            },
        })
        .unwrap();

        let s = render(&e.project);
        let a = analyze(&s);
        assert!(a.peak > 0.02, "{a:?}");
        assert!(a.duration_sec > 2.0);
    }

    #[test]
    fn solo_audio_clip_metronome_and_automation() {
        let mut e = Engine::default();
        let tone: Vec<f32> = (0..44100).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let asset = e.import_audio("tone", 44100, &tone);
        let a = e
            .apply(Command::AddTrack {
                name: "audio".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        e.apply(Command::AddClip {
            track: a,
            asset,
            start: 0,
            gain: None,
            offset_ticks: None,
            duration_ticks: None,
        })
        .unwrap();
        let i = e
            .apply(Command::AddTrack {
                name: "inst".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();
        e.apply(Command::AddNote {
            track: i,
            start: 0,
            duration: 480,
            pitch: 60,
            velocity: 100,
            lyric: None,
        })
        .unwrap();

        let both = analyze(&render(&e.project)).rms;
        // soloing the instrument must silence the audio clip (lower energy than both)
        e.apply(Command::SetMix {
            track: i,
            gain: None,
            pan: None,
            mute: None,
            solo: Some(true),
        })
        .unwrap();
        let solo = analyze(&render(&e.project)).rms;
        assert!(solo < both, "solo={solo} both={both}");

        // metronome adds energy
        let plain = analyze(&render(&e.project)).rms;
        let click = analyze(&render_with(&e.project, &RenderOptions { metronome: true })).rms;
        assert!(click > plain);

        // gain automation to zero silences the track
        e.apply(Command::AddAutoPoint {
            track: i,
            param: kymion_core::AutoParam::Gain,
            tick: 0,
            value: 0.0,
        })
        .unwrap();
        assert!(analyze(&render(&e.project)).peak < 1e-6);
    }

    #[test]
    fn true_peak_sees_intersample_overs() {
        // fs/4 sine at 45 degrees: every sample is +-0.7071 but the analog peak is 1.0.
        let n = 44100;
        let l: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin())
            .collect();
        let s = Stereo {
            sample_rate: 44100,
            left: l.clone(),
            right: l,
            limiter: None,
        };
        let r = calc_lufs(&s);
        assert!(
            r.true_peak_db > -0.3 && r.true_peak_db < 0.3,
            "true peak should be ~0 dBTP, got {}",
            r.true_peak_db
        );
        // never below the sample peak
        let sample_peak_db = 20.0 * std::f32::consts::FRAC_1_SQRT_2.log10();
        assert!(r.true_peak_db > sample_peak_db + 2.0);
    }

    #[test]
    fn true_peak_matches_sample_peak_for_smooth_signal_and_silence() {
        // A 100 Hz sine is heavily oversampled already: true peak == sample peak.
        let l: Vec<f32> = (0..44100)
            .map(|i| 0.5 * (std::f32::consts::TAU * 100.0 * i as f32 / 44100.0).sin())
            .collect();
        let s = Stereo {
            sample_rate: 44100,
            right: vec![0.0; l.len()],
            left: l,
            limiter: None,
        };
        let r = calc_lufs(&s);
        assert!(
            (r.true_peak_db - 20.0 * 0.5f32.log10()).abs() < 0.15,
            "{}",
            r.true_peak_db
        );
        let z = calc_lufs(&Stereo {
            sample_rate: 44100,
            left: vec![0.0; 4410],
            right: vec![0.0; 4410],
            limiter: None,
        });
        assert_eq!(z.true_peak_db, -120.0);
    }

    fn energy(id: u32, name: &str, frames: &[bool]) -> TrackBandEnergy {
        // band 3 (500-2000 Hz) loud on "on" frames, silence otherwise
        TrackBandEnergy {
            id,
            name: name.into(),
            band_rms: frames
                .iter()
                .map(|&on| {
                    if on {
                        [0.0, 0.0, 0.0, 0.05, 0.0, 0.0]
                    } else {
                        [0.0; 6]
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn clash_requires_a_contiguous_sustained_run() {
        // 20 overlapping frames, but scattered one at a time: not a sustained clash
        let scattered: Vec<bool> = (0..40).map(|i| i % 2 == 0).collect();
        let c = detect_frequency_clashes(&[energy(1, "a", &scattered), energy(2, "b", &scattered)]);
        assert!(c.is_empty(), "scattered frames must not count: {c:?}");

        // 12 contiguous frames (1.2 s) among silence: a real clash
        let mut run = vec![false; 5];
        run.extend(vec![true; 12]);
        run.extend(vec![false; 5]);
        let c = detect_frequency_clashes(&[energy(1, "a", &run), energy(2, "b", &run)]);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!((c[0].clash_duration_sec - 1.2).abs() < 1e-4);

        // a run that reaches the very end of the song is still counted
        let tail: Vec<bool> = (0..20).map(|i| i >= 5).collect();
        let c = detect_frequency_clashes(&[energy(1, "a", &tail), energy(2, "b", &tail)]);
        assert_eq!(c.len(), 1, "{c:?}");

        // a short run (0.9 s) is ignored
        let short: Vec<bool> = (0..20).map(|i| (3..12).contains(&i)).collect();
        assert!(
            detect_frequency_clashes(&[energy(1, "a", &short), energy(2, "b", &short)]).is_empty()
        );
    }

    const MINIMAL_SF2: &[u8] = include_bytes!("../tests/fixtures/minimal.sf2");

    #[test]
    fn soundfont_is_parsed_once_and_renders() {
        let mut e = Engine::default();
        let sf = e.import_soundfont("min", MINIMAL_SF2);
        let t = e
            .apply(Command::AddTrack {
                name: "sf".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();
        e.apply(Command::SetTrackSoundfont {
            track: t,
            soundfont: Some(SoundfontConfig {
                asset: sf,
                bank: 0,
                preset: 0,
            }),
        })
        .unwrap();

        let asset = &e.project.soundfonts[0];
        let first = load_soundfont(asset).expect("fixture is a valid SF2");
        let second = load_soundfont(asset).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "second load must hit the cache"
        );

        let s = render_note_preview(&e.project, t, 60, 0.3, 100);
        let peak = s.left.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak > 0.001, "SoundFont preview was silent: {peak}");

        // payloads that are not SoundFonts are rejected and never cached
        let bad = SoundfontAsset::from_bytes(99, "bad", b"not a soundfont");
        assert!(load_soundfont(&bad).is_none());
        let empty = SoundfontAsset::from_bytes(100, "empty", b"");
        assert!(load_soundfont(&empty).is_none());
        assert_eq!(list_soundfont_presets(MINIMAL_SF2).len(), 1);
    }

    #[test]
    fn metronome_click_count_matches_beats() {
        let mut e = Engine::default();
        let asset = e.import_audio("tone", 44100, &vec![0.0; 44100 * 4]);
        let a = e
            .apply(Command::AddTrack {
                name: "a".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        e.apply(Command::AddClip {
            track: a,
            asset,
            start: 0,
            gain: None,
            offset_ticks: None,
            duration_ticks: None,
        })
        .unwrap();
        // silent audio: any energy comes from the metronome alone
        let s = render_with(&e.project, &RenderOptions { metronome: true });
        let clicks = s
            .left
            .chunks(441)
            .filter(|c| c.iter().any(|x| x.abs() > 0.01))
            .count();
        assert!(
            clicks >= 8,
            "4 s at 120 bpm is >= 8 beats, got {clicks} loud blocks"
        );
    }

    #[test]
    fn pan_law_is_constant_power_and_symmetric() {
        let (l, r) = pan_gains(0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        for pan in [-1.0f32, -0.5, 0.0, 0.3, 1.0, 5.0, -5.0] {
            let (l, r) = pan_gains(pan);
            assert!((l * l + r * r - 1.0).abs() < 1e-5, "pan {pan}");
        }
        let (l, r) = pan_gains(-1.0);
        assert!(l > 0.999 && r < 1e-3);
    }

    #[test]
    fn overs_are_reported_despite_soft_limiter() {
        let mk = |tracks: usize| {
            let mut e = Engine::default();
            for _ in 0..tracks {
                let t = e
                    .apply(Command::AddTrack {
                        name: "s".into(),
                        kind: TrackKind::instrument(Waveform::Sine),
                    })
                    .unwrap()
                    .track_id
                    .unwrap();
                e.apply(Command::SetMix {
                    track: t,
                    gain: Some(2.0),
                    pan: None,
                    mute: None,
                    solo: None,
                })
                .unwrap();
                e.apply(Command::AddNote {
                    track: t,
                    start: 0,
                    duration: 1920,
                    pitch: 69,
                    velocity: 127,
                    lyric: None,
                })
                .unwrap();
            }
            analyze(&render(&e.project))
        };
        let quiet = mk(1);
        assert_eq!(quiet.clipped_samples, 0);
        assert!(quiet.pre_limiter_peak.unwrap() < 1.0);
        // four full-scale tracks sum to ~2.8 before the limiter: the output peak stays
        // below 1.0 but the overs must still be counted
        let loud = mk(4);
        assert!(loud.peak < 1.0, "limited output stays below full scale");
        assert!(loud.pre_limiter_peak.unwrap() > 1.5, "{loud:?}");
        assert!(loud.clipped_samples > 0, "{loud:?}");
    }

    #[test]
    fn wav_roundtrip() {
        let s = Stereo {
            sample_rate: 22050,
            left: vec![0.5; 100],
            right: vec![0.5; 100],
            limiter: None,
        };
        let (sr, mono) = decode_wav(&to_wav_bytes(&s)).unwrap();
        assert_eq!(sr, 22050);
        assert_eq!(mono.len(), 100);
        assert!((mono[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn clip_trim_fade_and_metering() {
        let mut e = Engine::default();
        let samples = vec![1.0f32; 44100]; // 1 second DC = 1.0
        let asset = e.import_audio("dc", 44100, &samples);
        let tr = e
            .apply(Command::AddTrack {
                name: "A".into(),
                kind: TrackKind::Audio,
            })
            .unwrap()
            .track_id
            .unwrap();
        let cl = e
            .apply(Command::AddClip {
                track: tr,
                asset,
                start: 0,
                gain: Some(1.0),
                offset_ticks: None,
                duration_ticks: None,
            })
            .unwrap()
            .clip_id
            .unwrap();

        // Apply fade in (half second = 480 ticks) and fade out (480 ticks)
        e.apply(Command::SetClipFade {
            track: tr,
            clip: cl,
            fade_in_ticks: Some(480),
            fade_out_ticks: Some(480),
        })
        .unwrap();

        let full = render_full(&e.project, &RenderOptions::default());
        assert_eq!(full.track_meters.len(), 1);
        let m = full.meters_at(0, 100);
        // Start of fade-in is near 0
        assert!(m.master.left.peak < 0.05);

        // Mid-point should have healthy level
        let mid = full.meters_at(22050, 100);
        assert!(mid.master.left.peak > 0.5);

        // Check track meter is populated
        assert!(m.tracks.contains_key(&tr));
    }

    #[test]
    fn synth_ladder_and_lufs_diagnostics() {
        let mut e = Engine::default();
        let t1 = e
            .apply(Command::AddTrack {
                name: "Moog Bass".into(),
                kind: TrackKind::synth(
                    Waveform::Saw,
                    SynthConfig {
                        filter: SynthFilter::Moog,
                        cutoff_hz: 600.0,
                        resonance: 0.7,
                        filter_env_amount: 0.6,
                        attack: 0.005,
                        decay: 0.15,
                        sustain: 0.3,
                        release: 0.2,
                    },
                ),
            })
            .unwrap()
            .track_id
            .unwrap();

        // Add 8 quarter notes
        for beat in 0..8 {
            e.apply(Command::AddNote {
                track: t1,
                start: beat * 480,
                duration: 460,
                pitch: 36, // C2 bass note
                velocity: 110,
                lyric: None,
            })
            .unwrap();
        }

        let s = render(&e.project);
        let a = analyze(&s);
        assert!(a.peak > 0.05);
        assert!(a.lufs.is_some());
        let lufs = a.lufs.unwrap();
        assert!(lufs.integrated.is_finite());
        assert!(lufs.true_peak_db.is_finite());

        // Add a second competing bass track playing in the exact same frequency range
        let t2 = e
            .apply(Command::AddTrack {
                name: "Sub Synth".into(),
                kind: TrackKind::instrument(Waveform::Sine),
            })
            .unwrap()
            .track_id
            .unwrap();

        for beat in 0..8 {
            e.apply(Command::AddNote {
                track: t2,
                start: beat * 480,
                duration: 460,
                pitch: 38, // D2 bass note
                velocity: 110,
                lyric: None,
            })
            .unwrap();
        }

        let diag = diagnose(&e.project);
        assert!(!diag.summary.is_empty());
        // Verify frequency clashes are detected between the two competing bass tracks
        assert!(!diag.clashes.is_empty());
        assert_eq!(diag.clashes[0].track_a_id, t1);
        assert_eq!(diag.clashes[0].track_b_id, t2);
        assert!(diag.clashes[0].suggestion.contains("compete"));
    }

    #[test]
    fn note_preview_audition() {
        let mut e = Engine::default();
        let t = e
            .apply(Command::AddTrack {
                name: "Synth".into(),
                kind: TrackKind::synth(
                    Waveform::Saw,
                    SynthConfig {
                        filter: SynthFilter::Moog,
                        cutoff_hz: 1000.0,
                        resonance: 0.5,
                        filter_env_amount: 0.5,
                        attack: 0.005,
                        decay: 0.1,
                        sustain: 0.5,
                        release: 0.1,
                    },
                ),
            })
            .unwrap()
            .track_id
            .unwrap();

        let s = render_note_preview(&e.project, t, 60, 0.3, 100);
        assert_eq!(s.sample_rate, 44100);
        assert!(!s.left.is_empty());
        assert_eq!(s.left.len(), s.right.len());
        let peak = s.left.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak > 0.01, "Preview sound output was silent: {peak}");
    }
}
