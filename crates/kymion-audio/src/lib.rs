//! Kymion audio engine: offline renderer, mixer, analysis, WAV export.

use kymion_core::{Project, Track, TrackKind, Waveform};
use kymion_dsp::{midi_to_hz, Adsr, Osc};
use kymion_vocal::{render_track as render_vocal, FormantVoice};
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Stereo { pub sample_rate: u32, pub left: Vec<f32>, pub right: Vec<f32> }

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub duration_sec: f32,
    pub peak: f32,
    pub rms: f32,
    pub clipped_samples: usize,
}

fn render_instrument(p: &Project, t: &Track, wf: Waveform, sr: u32) -> Vec<f32> {
    let srf = sr as f32;
    let total = (p.ticks_to_secs(p.length_ticks()) * srf) as usize + sr as usize / 2;
    let mut buf = vec![0f32; total];
    for n in &t.notes {
        let dur = p.ticks_to_secs(n.duration);
        let len = ((dur + 0.25) * srf) as usize;
        let off = (p.ticks_to_secs(n.start) * srf) as usize;
        let hz = midi_to_hz(n.pitch as f32);
        let mut osc = Osc::default();
        let mut env = match wf {
            Waveform::Pluck => Adsr::new(0.002, 0.35, 0.0, 0.1, srf),
            _ => Adsr::new(0.01, 0.1, 0.7, 0.2, srf),
        };
        let vel = n.velocity as f32 / 127.0;
        for i in 0..len {
            if i as f32 / srf >= dur { env.gate_off(); }
            let s = match wf {
                Waveform::Sine => osc.sine(hz, srf),
                Waveform::Saw => osc.saw(hz, srf) * 0.5,
                Waveform::Square => osc.square(hz, srf) * 0.4,
                Waveform::Triangle => osc.triangle(hz, srf),
                Waveform::Pluck => osc.saw(hz, srf) * 0.5,
            };
            if let Some(b) = buf.get_mut(off + i) { *b += s * env.next() * vel * 0.5; }
        }
    }
    buf
}

/// Render the whole project to stereo.
pub fn render(p: &Project) -> Stereo {
    let sr = p.sample_rate;
    let mut l: Vec<f32> = vec![];
    let mut r: Vec<f32> = vec![];
    for t in p.tracks.iter().filter(|t| !t.mute) {
        let mono = match &t.kind {
            TrackKind::Instrument { waveform } => render_instrument(p, t, *waveform, sr),
            TrackKind::Vocal { .. } => render_vocal(p, t, &FormantVoice, sr),
        };
        if l.len() < mono.len() { l.resize(mono.len(), 0.0); r.resize(mono.len(), 0.0); }
        // equal-power pan
        let a = (t.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (a.cos() * t.gain, a.sin() * t.gain);
        for (i, s) in mono.iter().enumerate() { l[i] += s * gl; r[i] += s * gr; }
    }
    // soft limiter
    for s in l.iter_mut().chain(r.iter_mut()) { *s = s.tanh(); }
    Stereo { sample_rate: sr, left: l, right: r }
}

pub fn analyze(s: &Stereo) -> Analysis {
    let n = s.left.len().max(1);
    let (mut peak, mut sum, mut clip) = (0f32, 0f64, 0usize);
    for x in s.left.iter().chain(s.right.iter()) {
        peak = peak.max(x.abs());
        sum += (*x as f64).powi(2);
        if x.abs() >= 0.999 { clip += 1; }
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
    let spec = hound::WavSpec { channels: 2, sample_rate: s.sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
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
    s.left.iter().zip(&s.right).flat_map(|(a, b)| [*a, *b]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kymion_core::*;
    #[test]
    fn renders_non_silent() {
        let mut e = Engine::default();
        let t = e.apply(Command::AddTrack { name: "v".into(), kind: TrackKind::Vocal { voice: "default".into() } }).unwrap().track_id.unwrap();
        e.apply(Command::AddNote { track: t, start: 0, duration: 480, pitch: 64, velocity: 100, lyric: Some("라".into()) }).unwrap();
        let a = analyze(&render(&e.project));
        assert!(a.peak > 0.05, "{a:?}");
    }
}
