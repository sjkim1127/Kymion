//! Kymion audio engine: offline renderer, mixer, DSP effects chain, analysis, WAV export.

use kymion_core::{eval_auto, FxInstance, FxKind, Project, Track, TrackKind, Waveform, PPQ};
use kymion_dsp::{midi_to_hz, Adsr, Compressor, Eq3Band, Osc, StereoDelay, StereoReverb};
use kymion_vocal::{render_track as render_vocal, FormantVoice};
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Stereo {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub duration_sec: f32,
    pub peak: f32,
    pub rms: f32,
    pub clipped_samples: usize,
}

fn render_instrument_into(p: &Project, t: &Track, wf: Waveform, sr: u32, buf: &mut [f32]) {
    let srf = sr as f32;
    for n in &t.notes {
        let dur = p.ticks_to_secs(n.duration);
        let len = ((dur + 0.35) * srf) as usize;
        let off = (p.ticks_to_secs(n.start) * srf) as usize;
        let hz = midi_to_hz(n.pitch as f32);
        let mut osc = Osc::default();
        let mut env = match wf {
            Waveform::Pluck => Adsr::new(0.002, 0.4, 0.0, 0.15, srf),
            _ => Adsr::new(0.01, 0.1, 0.7, 0.25, srf),
        };
        let vel = n.velocity as f32 / 127.0;
        let note_end = (off + len).min(buf.len());
        let count = note_end.saturating_sub(off);
        for i in 0..count {
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
            buf[off + i] += s * env.next() * vel * 0.5;
        }
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
                let mut eq_l = Eq3Band::new(*low_gain, *mid_gain, *mid_freq, *mid_q, *high_gain, srf);
                let mut eq_r = Eq3Band::new(*low_gain, *mid_gain, *mid_freq, *mid_q, *high_gain, srf);
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
                let mut comp = Compressor::new(*threshold_db, *ratio, *attack_ms, *release_ms, *makeup_db, srf);
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
        let Some(asset) = p.assets.iter().find(|a| a.id == clip.asset) else { continue };
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
    let mut b = 0u32;
    loop {
        let tick = b * beat;
        let start = (p.ticks_to_secs(tick) * srf) as usize;
        if start >= l.len() || tick > p.length_ticks() + p.bar_ticks() {
            break;
        }
        let down = b % p.time_sig_num.max(1) as u32 == 0;
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
        let num_blocks = (n + METER_BLOCK_SIZE - 1) / METER_BLOCK_SIZE;
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
        let end_blk = ((start + len + METER_BLOCK_SIZE - 1) / METER_BLOCK_SIZE).min(self.peaks.len());
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
pub struct FullRender {
    pub master: Stereo,
    pub track_meters: Vec<TrackMeterEnvelope>,
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
            master: StereoMeter { left: ml, right: mr },
            tracks,
        }
    }
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
    let any_solo = p.tracks.iter().any(|t| t.solo);
    let ticks_per_sample = p.tempo * PPQ as f32 / 60.0 / srf;

    for t in p.tracks.iter().filter(|t| !t.mute && (!any_solo || t.solo)) {
        mono.fill(0.0);
        match &t.kind {
            TrackKind::Instrument { waveform } => render_instrument_into(p, t, *waveform, sr, &mut mono),
            TrackKind::Vocal { .. } => {
                let v = render_vocal(p, t, &FormantVoice, sr);
                let copy_len = v.len().min(total_samples);
                mono[..copy_len].copy_from_slice(&v[..copy_len]);
            }
            TrackKind::Audio => render_audio_track_into(p, t, sr, &mut mono),
        };

        trk_l.fill(0.0);
        trk_r.fill(0.0);

        let static_pan = {
            let a = (t.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            (a.cos(), a.sin())
        };
        let automated = !t.gain_auto.is_empty() || !t.pan_auto.is_empty();

        for i in 0..total_samples {
            let s = mono[i];
            let (gain, (pl, pr)) = if automated {
                let tick = i as f32 * ticks_per_sample;
                let g = eval_auto(&t.gain_auto, tick, t.gain);
                let pan = eval_auto(&t.pan_auto, tick, t.pan);
                let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
                (g, (a.cos(), a.sin()))
            } else {
                (t.gain, static_pan)
            };
            trk_l[i] = s * gain * pl;
            trk_r[i] = s * gain * pr;
        }

        // Apply track FX insert chain
        apply_fx_chain(&mut trk_l, &mut trk_r, &t.fx, sr);

        for i in 0..total_samples {
            master_l[i] += trk_l[i];
            master_r[i] += trk_r[i];
        }

        track_meters.push(TrackMeterEnvelope::from_audio(t.id, &trk_l, &trk_r));
    }

    // Apply master bus FX chain
    apply_fx_chain(&mut master_l, &mut master_r, &p.master_fx, sr);

    if opts.metronome {
        add_metronome(p, &mut master_l, &mut master_r, sr);
    }

    // Soft limiter on master output
    for s in master_l.iter_mut().chain(master_r.iter_mut()) {
        *s = s.tanh();
    }

    FullRender {
        master: Stereo { sample_rate: sr, left: master_l, right: master_r },
        track_meters,
    }
}

/// Decode WAV bytes (8/16/24/32-bit int or 32-bit float) to mono f32 + sample rate.
pub fn decode_wav(bytes: &[u8]) -> Result<(u32, Vec<f32>), String> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|v| v as f32 / max).collect()
        }
    };
    let mono = raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok((spec.sample_rate, mono))
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
    Analysis {
        duration_sec: n as f32 / s.sample_rate as f32,
        peak,
        rms: (sum / (2 * n) as f64).sqrt() as f32,
        clipped_samples: clip,
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
            w.write_sample((a.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
            w.write_sample((b.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
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
        let a = e.apply(Command::AddTrack { name: "audio".into(), kind: TrackKind::Audio }).unwrap().track_id.unwrap();
        e.apply(Command::AddClip { track: a, asset, start: 0, gain: None, offset_ticks: None, duration_ticks: None }).unwrap();
        let i = e.apply(Command::AddTrack { name: "inst".into(), kind: TrackKind::Instrument { waveform: Waveform::Sine } }).unwrap().track_id.unwrap();
        e.apply(Command::AddNote { track: i, start: 0, duration: 480, pitch: 60, velocity: 100, lyric: None }).unwrap();

        let both = analyze(&render(&e.project)).rms;
        // soloing the instrument must silence the audio clip (lower energy than both)
        e.apply(Command::SetMix { track: i, gain: None, pan: None, mute: None, solo: Some(true) }).unwrap();
        let solo = analyze(&render(&e.project)).rms;
        assert!(solo < both, "solo={solo} both={both}");

        // metronome adds energy
        let plain = analyze(&render(&e.project)).rms;
        let click = analyze(&render_with(&e.project, &RenderOptions { metronome: true })).rms;
        assert!(click > plain);

        // gain automation to zero silences the track
        e.apply(Command::AddAutoPoint { track: i, param: kymion_core::AutoParam::Gain, tick: 0, value: 0.0 }).unwrap();
        assert!(analyze(&render(&e.project)).peak < 1e-6);
    }

    #[test]
    fn wav_roundtrip() {
        let s = Stereo { sample_rate: 22050, left: vec![0.5; 100], right: vec![0.5; 100] };
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
        let tr = e.apply(Command::AddTrack { name: "A".into(), kind: TrackKind::Audio }).unwrap().track_id.unwrap();
        let cl = e.apply(Command::AddClip {
            track: tr,
            asset,
            start: 0,
            gain: Some(1.0),
            offset_ticks: None,
            duration_ticks: None,
        }).unwrap().clip_id.unwrap();

        // Apply fade in (half second = 480 ticks) and fade out (480 ticks)
        e.apply(Command::SetClipFade { track: tr, clip: cl, fade_in_ticks: Some(480), fade_out_ticks: Some(480) }).unwrap();

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
}
