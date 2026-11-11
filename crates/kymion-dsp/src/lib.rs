//! Kymion DSP primitives: oscillators, envelopes, filters, noise.

use std::f32::consts::TAU;

pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Phase-accumulator oscillator with PolyBLEP anti-aliasing for saw/square.
#[derive(Clone, Default)]
pub struct Osc {
    phase: f32,
}
impl Osc {
    fn polyblep(t: f32, dt: f32) -> f32 {
        if t < dt { let x = t / dt; x + x - x * x - 1.0 }
        else if t > 1.0 - dt { let x = (t - 1.0) / dt; x * x + x + x + 1.0 }
        else { 0.0 }
    }
    pub fn sine(&mut self, hz: f32, sr: f32) -> f32 {
        let v = (self.phase * TAU).sin();
        self.advance(hz, sr);
        v
    }
    pub fn saw(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = hz / sr;
        let v = 2.0 * self.phase - 1.0 - Self::polyblep(self.phase, dt);
        self.advance(hz, sr);
        v
    }
    pub fn square(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = hz / sr;
        let mut v = if self.phase < 0.5 { 1.0 } else { -1.0 };
        v += Self::polyblep(self.phase, dt);
        v -= Self::polyblep((self.phase + 0.5) % 1.0, dt);
        self.advance(hz, sr);
        v
    }
    pub fn triangle(&mut self, hz: f32, sr: f32) -> f32 {
        let v = 4.0 * (self.phase - 0.5).abs() - 1.0;
        self.advance(hz, sr);
        v
    }
    /// Glottal-like pulse: band-rich source used for voice synthesis.
    pub fn glottal(&mut self, hz: f32, sr: f32) -> f32 {
        let p = self.phase;
        let v = if p < 0.6 { (p / 0.6 * std::f32::consts::PI).sin() } else { -0.3 * ((p - 0.6) / 0.4 * std::f32::consts::PI).sin() };
        self.advance(hz, sr);
        v
    }
    fn advance(&mut self, hz: f32, sr: f32) {
        self.phase += hz / sr;
        if self.phase >= 1.0 { self.phase -= 1.0; }
    }
}

/// Linear ADSR; call `gate_off` to release.
#[derive(Clone)]
pub struct Adsr { a: f32, d: f32, s: f32, r: f32, level: f32, stage: u8, sr: f32 }
impl Adsr {
    pub fn new(a: f32, d: f32, s: f32, r: f32, sr: f32) -> Self { Self { a, d, s, r, level: 0.0, stage: 0, sr } }
    pub fn gate_off(&mut self) { if self.stage < 3 { self.stage = 3; } }
    pub fn next(&mut self) -> f32 {
        match self.stage {
            0 => { self.level += 1.0 / (self.a * self.sr).max(1.0); if self.level >= 1.0 { self.level = 1.0; self.stage = 1; } }
            1 => { self.level -= (1.0 - self.s) / (self.d * self.sr).max(1.0); if self.level <= self.s { self.level = self.s; self.stage = 2; } }
            2 => {}
            _ => { self.level -= self.s.max(0.001) / (self.r * self.sr).max(1.0); if self.level < 0.0 { self.level = 0.0; } }
        }
        self.level
    }
}

/// Resonant two-pole bandpass (RBJ biquad), used as a formant resonator.
#[derive(Clone, Default)]
pub struct Biquad { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32, z1: f32, z2: f32 }
impl Biquad {
    pub fn bandpass(freq: f32, q: f32, sr: f32) -> Self {
        let mut b = Self::default();
        b.set_bandpass(freq, q, sr);
        b
    }
    pub fn set_bandpass(&mut self, freq: f32, q: f32, sr: f32) {
        let w = TAU * freq.min(sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0; self.b1 = 0.0; self.b2 = -alpha / a0;
        self.a1 = -2.0 * w.cos() / a0; self.a2 = (1.0 - alpha) / a0;
    }
    pub fn lowpass(freq: f32, q: f32, sr: f32) -> Self {
        let w = TAU * freq.min(sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let c = w.cos();
        Self { b0: (1.0 - c) / 2.0 / a0, b1: (1.0 - c) / a0, b2: (1.0 - c) / 2.0 / a0, a1: -2.0 * c / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// xorshift white noise (deterministic, no deps).
#[derive(Clone)]
pub struct Noise(u32);
impl Noise {
    pub fn new(seed: u32) -> Self { Self(seed.max(1)) }
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 17; self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a4_is_440() { assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3); }
    #[test]
    fn osc_bounded() {
        let mut o = Osc::default();
        for _ in 0..1000 { assert!(o.saw(440.0, 44100.0).abs() <= 1.5); }
    }
}
