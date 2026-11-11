//! Kymion audio engine: offline renderer, mixer, DSP effects chain, analysis, WAV export.

use kymion_core::{FxInstance, FxKind, Project, Track, TrackKind, Waveform};
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

/// Render the whole project to stereo including track and master FX chains.
pub fn render(p: &Project) -> Stereo {
    let sr = p.sample_rate;
    let srf = sr as f32;
    // Tail room for delays/reverbs (extra 2 seconds)
    let total_samples = (p.ticks_to_secs(p.length_ticks()) * srf) as usize + sr as usize * 2;

    let mut master_l: Vec<f32> = vec![0.0; total_samples];
    let mut master_r: Vec<f32> = vec![0.0; total_samples];

    for t in p.tracks.iter().filter(|t| !t.mute) {
        let mono = match &t.kind {
            TrackKind::Instrument { waveform } => render_instrument(p, t, *waveform, sr),
            TrackKind::Vocal { .. } => render_vocal(p, t, &FormantVoice, sr),
        };

        let mut trk_l = vec![0.0; total_samples];
        let mut trk_r = vec![0.0; total_samples];

        // Equal-power pan
        let a = (t.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (a.cos() * t.gain, a.sin() * t.gain);

        for (i, &s) in mono.iter().enumerate() {
            if i < total_samples {
                trk_l[i] = s * gl;
                trk_r[i] = s * gr;
            }
        }

        // Apply track FX insert chain
        apply_fx_chain(&mut trk_l, &mut trk_r, &t.fx, sr);

        // Sum into master bus
        for i in 0..total_samples {
            master_l[i] += trk_l[i];
            master_r[i] += trk_r[i];
        }
    }

    // Apply master bus FX chain
    apply_fx_chain(&mut master_l, &mut master_r, &p.master_fx, sr);

    // Soft limiter on master output
    for s in master_l.iter_mut().chain(master_r.iter_mut()) {
        *s = s.tanh();
    }

    Stereo {
        sample_rate: sr,
        left: master_l,
        right: master_r,
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
}
