//! Kymion vocal: lyric → phoneme → formant singing synthesis with
//! vocaloid-style pitch control (portamento, vibrato, formant shift, breath).
//!
//! This is the v0.1 rule-based engine. The [`Voice`] seam is where a
//! concatenative / neural voicebank backend plugs in later.

use kymion_core::{Expression, Note, Project, Track};
use kymion_dsp::{midi_to_hz, Adsr, Biquad, Noise, Osc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vowel {
    A,
    E,
    I,
    O,
    U,
    Eu,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Onset {
    None,
    Stop,
    Fricative,
    Nasal,
}

#[derive(Clone, Copy, Debug)]
pub struct Phoneme {
    pub onset: Onset,
    pub vowel: Vowel,
}

impl Vowel {
    /// Approximate F1/F2/F3 (Hz) for a neutral adult voice.
    pub fn formants(self) -> [f32; 3] {
        match self {
            Vowel::A => [800.0, 1150.0, 2800.0],
            Vowel::E => [530.0, 1840.0, 2480.0],
            Vowel::I => [270.0, 2290.0, 3010.0],
            Vowel::O => [570.0, 840.0, 2410.0],
            Vowel::U => [300.0, 870.0, 2240.0],
            Vowel::Eu => [330.0, 1200.0, 2400.0],
        }
    }
}

/// Convert one lyric syllable (Hangul / kana / Latin) to a phoneme.
pub fn lyric_to_phoneme(lyric: &str) -> Phoneme {
    for c in lyric.chars() {
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) {
            let idx = u - 0xAC00;
            let (cho, jung) = (idx / 588, (idx % 588) / 28);
            let vowel = match jung {
                0..=3 | 9 | 10 => Vowel::A,
                4..=7 => Vowel::E,
                8 | 11 | 12 => Vowel::O,
                13 | 14 | 15 | 17 => Vowel::U,
                16 | 20 => Vowel::I,
                _ => Vowel::Eu,
            };
            let onset = match cho {
                11 => Onset::None,
                2 | 6 | 5 => Onset::Nasal,
                9 | 10 | 12 | 13 | 14 | 18 => Onset::Fricative,
                _ => Onset::Stop,
            };
            return Phoneme { onset, vowel };
        }
        let k = if (0x30A1..=0x30F6).contains(&u) {
            char::from_u32(u - 0x60).unwrap()
        } else {
            c
        };
        if let Some(v) = kana_vowel(k) {
            let onset = if "あいうえお".contains(k) {
                Onset::None
            } else if "なにぬねのまみむめも".contains(k) {
                Onset::Nasal
            } else if "さしすせそはひふへほざじずぜぞ".contains(k) {
                Onset::Fricative
            } else {
                Onset::Stop
            };
            return Phoneme { onset, vowel: v };
        }
        if let Some(v) = match c.to_ascii_lowercase() {
            'a' => Some(Vowel::A),
            'e' => Some(Vowel::E),
            'i' => Some(Vowel::I),
            'o' => Some(Vowel::O),
            'u' => Some(Vowel::U),
            _ => None,
        } {
            let onset = match lyric.chars().next().map(|x| x.to_ascii_lowercase()) {
                Some(x) if "aeiou".contains(x) => Onset::None,
                Some(x) if "mn".contains(x) => Onset::Nasal,
                Some(x) if "sfhzj".contains(x) => Onset::Fricative,
                _ => Onset::Stop,
            };
            return Phoneme { onset, vowel: v };
        }
    }
    Phoneme {
        onset: Onset::None,
        vowel: Vowel::A,
    }
}

fn kana_vowel(c: char) -> Option<Vowel> {
    const ROWS: [(&str, Vowel); 5] = [
        ("あかさたなはまやらわがざだばぱぁゃ", Vowel::A),
        ("いきしちにひみりぎじぢびぴぃ", Vowel::I),
        ("うくすつぬふむゆるぐずづぶぷぅゅ", Vowel::U),
        ("えけせてねへめれげぜでべぺぇ", Vowel::E),
        ("おこそとのほもよろをごぞどぼぽぉょ", Vowel::O),
    ];
    ROWS.iter().find(|(s, _)| s.contains(c)).map(|(_, v)| *v)
}

/// Pluggable singing backend.
pub trait Voice {
    fn render_note(&self, req: &NoteRequest, sr: u32) -> Vec<f32>;
}

pub struct NoteRequest {
    pub phoneme: Phoneme,
    /// MIDI pitch (fractional) at note start (for portamento) and target.
    pub from_pitch: f32,
    pub pitch: f32,
    pub velocity: f32,
    pub seconds: f32,
    pub expr: Expression,
}

/// Source-filter formant voice.
pub struct FormantVoice;

impl Voice for FormantVoice {
    fn render_note(&self, r: &NoteRequest, sr: u32) -> Vec<f32> {
        let srf = sr as f32;
        let rel = 0.08;
        let n = ((r.seconds + rel) * srf) as usize;
        let mut out = Vec::with_capacity(n);
        let shift = 2f32.powf(r.expr.formant_shift / 12.0);
        let f = r.phoneme.vowel.formants();
        let bw = [80.0, 100.0, 140.0];
        let mut filters: Vec<Biquad> = (0..3)
            .map(|i| Biquad::bandpass(f[i] * shift, f[i] * shift / bw[i], srf))
            .collect();
        let gains = [1.0, 0.6, 0.3];
        let mut osc = Osc::default();
        let mut noise = Noise::new(0x9E3779B9);
        let mut hiss = Biquad::bandpass(6000.0, 1.0, srf);
        let mut env = Adsr::new(0.015, 0.05, 0.85, rel, srf);
        let mut vib_phase = 0f32;
        let (onset_len, onset_gain) = match r.phoneme.onset {
            Onset::None => (0.0, 0.0),
            Onset::Stop => (0.012, 0.6),
            Onset::Fricative => (0.045, 0.5),
            Onset::Nasal => (0.03, 0.0),
        };
        for i in 0..n {
            let t = i as f32 / srf;
            if t >= r.seconds {
                env.gate_off();
            }
            // pitch: portamento glide + delayed vibrato
            let glide = if r.expr.portamento > 0.0 {
                (t / r.expr.portamento).min(1.0)
            } else {
                1.0
            };
            let mut pitch = r.from_pitch + (r.pitch - r.from_pitch) * glide;
            if t > r.expr.vibrato_delay {
                let fade = ((t - r.expr.vibrato_delay) / 0.2).min(1.0);
                vib_phase += r.expr.vibrato_rate / srf;
                pitch +=
                    (vib_phase * std::f32::consts::TAU).sin() * r.expr.vibrato_depth / 100.0 * fade;
            }
            let hz = midi_to_hz(pitch);
            let src =
                osc.glottal(hz, srf) * (1.0 - r.expr.breath) + noise.next() * r.expr.breath * 0.5;
            let mut v = 0.0;
            for (b, g) in filters.iter_mut().zip(gains) {
                v += b.process(src) * g;
            }
            v *= 2.5;
            if t < onset_len {
                let k = 1.0 - t / onset_len;
                v = v * (0.3 + 0.7 * (1.0 - k)) + hiss.process(noise.next()) * onset_gain * k * 2.0;
            }
            out.push(v * env.next() * r.velocity);
        }
        out
    }
}

/// Render a vocal track to mono samples at `sr`.
pub fn render_track(project: &Project, track: &Track, voice: &dyn Voice, sr: u32) -> Vec<f32> {
    let total =
        (project.ticks_to_secs(project.length_ticks()) * sr as f32) as usize + sr as usize / 2;
    let mut buf = vec![0f32; total];
    let mut prev: Option<(&Note, u32)> = None;
    for note in &track.notes {
        let expr = note
            .expression
            .clone()
            .unwrap_or_else(|| track.expression.clone());
        let from = match prev {
            Some((p, end)) if note.start <= end.saturating_add(60) => p.pitch as f32,
            _ => note.pitch as f32,
        };
        let req = NoteRequest {
            phoneme: lyric_to_phoneme(note.lyric.as_deref().unwrap_or("la")),
            from_pitch: from,
            pitch: note.pitch as f32,
            velocity: note.velocity as f32 / 127.0,
            seconds: project.ticks_to_secs(note.duration),
            expr,
        };
        let samples = voice.render_note(&req, sr);
        let off = (project.ticks_to_secs(note.start) * sr as f32) as usize;
        for (i, s) in samples.iter().enumerate() {
            if let Some(b) = buf.get_mut(off + i) {
                *b += s;
            }
        }
        prev = Some((note, note.start.saturating_add(note.duration)));
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hangul() {
        assert_eq!(lyric_to_phoneme("사").vowel, Vowel::A);
        assert_eq!(lyric_to_phoneme("사").onset, Onset::Fricative);
        assert_eq!(lyric_to_phoneme("랑").vowel, Vowel::A);
        assert_eq!(lyric_to_phoneme("이").vowel, Vowel::I);
    }
    #[test]
    fn latin_and_kana() {
        assert_eq!(lyric_to_phoneme("mo").vowel, Vowel::O);
        assert_eq!(lyric_to_phoneme("キ").vowel, Vowel::I);
    }
}
