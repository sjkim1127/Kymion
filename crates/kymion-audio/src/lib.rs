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

fn render_instrument(p: &Project, t: &Track, wf: Waveform, sr: u32) -> Vec<f32> {
    let srf = sr as f32;
    // Base length + 2 seconds release/decay room
    let total = (p.ticks_to_secs(p.length_ticks()) * srf) as usize + sr as usize * 2;
    let mut buf = vec![0f32; total];
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
        for i in 0..len {
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
            if let Some(b) = buf.get_mut(off + i) {
                *b += s * env.next() * vel * 0.5;
            }
        }
    }
    buf
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

/// Mono audio track: place every clip's asset (linear-resampled) on the timeline.
fn render_audio_track(p: &Project, t: &Track, sr: u32, total: usize) -> Vec<f32> {
    let mut buf = vec![0f32; total];
    for clip in &t.clips {
        let Some(asset) = p.assets.iter().find(|a| a.id == clip.asset) else { continue };
        let data = asset.samples();
        if data.is_empty() {
            continue;
        }
        let ratio = asset.sample_rate as f32 / sr as f32;
        let off = (p.ticks_to_secs(clip.start) * sr as f32) as usize;
        let out_len = (data.len() as f32 / ratio) as usize;
        for i in 0..out_len {
            let pos = i as f32 * ratio;
            let k = pos as usize;
            let frac = pos - k as f32;
            let a = data[k.min(data.len() - 1)];
            let b = data[(k + 1).min(data.len() - 1)];
            if let Some(o) = buf.get_mut(off + i) {
                *o += (a + (b - a) * frac) * clip.gain;
            }
        }
    }
    buf
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

/// Render the whole project to stereo including track and master FX chains.
pub fn render(p: &Project) -> Stereo {
    render_with(p, &RenderOptions::default())
}

pub fn render_with(p: &Project, opts: &RenderOptions) -> Stereo {
    let sr = p.sample_rate;
    let srf = sr as f32;
    // Tail room for delays/reverbs (extra 2 seconds)
    let total_samples = (p.ticks_to_secs(p.length_ticks()) * srf) as usize + sr as usize * 2;

    let mut master_l: Vec<f32> = vec![0.0; total_samples];
    let mut master_r: Vec<f32> = vec![0.0; total_samples];
    let any_solo = p.tracks.iter().any(|t| t.solo);
    let ticks_per_sample = p.tempo * PPQ as f32 / 60.0 / srf;

    for t in p.tracks.iter().filter(|t| !t.mute && (!any_solo || t.solo)) {
        let mono = match &t.kind {
            TrackKind::Instrument { waveform } => render_instrument(p, t, *waveform, sr),
            TrackKind::Vocal { .. } => render_vocal(p, t, &FormantVoice, sr),
            TrackKind::Audio => render_audio_track(p, t, sr, total_samples),
        };

        let mut trk_l = vec![0.0; total_samples];
        let mut trk_r = vec![0.0; total_samples];

        let static_pan = {
            let a = (t.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            (a.cos(), a.sin())
        };
        let automated = !t.gain_auto.is_empty() || !t.pan_auto.is_empty();

        for (i, &s) in mono.iter().enumerate() {
            if i >= total_samples {
                break;
            }
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

    Stereo { sample_rate: sr, left: master_l, right: master_r }
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
        e.apply(Command::AddClip { track: a, asset, start: 0, gain: None }).unwrap();
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
}
