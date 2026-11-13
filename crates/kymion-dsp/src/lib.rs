//! Kymion DSP primitives: oscillators, envelopes, filters, noise, and audio effects.

#![allow(clippy::excessive_precision, clippy::should_implement_trait)]

use std::f32::consts::TAU;

pub const MIDI_PITCH_TABLE: [f32; 128] = [
    8.175799,
    8.661957,
    9.177024,
    9.722718,
    10.300861,
    10.913382,
    11.562326,
    12.249857,
    12.978272,
    13.750000,
    14.567618,
    15.433853,
    16.351598,
    17.323914,
    18.354048,
    19.445436,
    20.601722,
    21.826764,
    23.124651,
    24.499715,
    25.956544,
    27.500000,
    29.135235,
    30.867706,
    32.703196,
    34.647829,
    36.708096,
    38.890873,
    41.203445,
    43.653529,
    46.249303,
    48.999429,
    51.913087,
    55.000000,
    58.270470,
    61.735413,
    65.406391,
    69.295658,
    73.416192,
    77.781746,
    82.406889,
    87.307058,
    92.498606,
    97.998859,
    103.826174,
    110.000000,
    116.540940,
    123.470825,
    130.812783,
    138.591315,
    146.832384,
    155.563492,
    164.813778,
    174.614116,
    184.997211,
    195.997718,
    207.652349,
    220.000000,
    233.081881,
    246.941651,
    261.625565,
    277.182631,
    293.664768,
    311.126984,
    329.627557,
    349.228231,
    369.994423,
    391.995436,
    415.304698,
    440.000000,
    466.163762,
    493.883301,
    523.251131,
    554.365262,
    587.329536,
    622.253967,
    659.255114,
    698.456463,
    739.988845,
    783.990872,
    830.609395,
    880.000000,
    932.327523,
    987.766603,
    1046.502261,
    1108.730524,
    1174.659072,
    1244.507935,
    1318.510228,
    1396.912926,
    1479.977691,
    1567.981744,
    1661.218790,
    1760.000000,
    1864.655046,
    1975.533205,
    2093.004522,
    2217.461048,
    2349.318143,
    2489.015870,
    2637.020455,
    2793.825851,
    2959.955382,
    3135.963488,
    3322.437581,
    3520.000000,
    3729.310092,
    3951.066410,
    4186.009045,
    4434.922096,
    4698.636287,
    4978.031740,
    5274.040911,
    5587.651703,
    5919.910763,
    6271.926976,
    6644.875161,
    7040.000000,
    7458.620184,
    7902.132820,
    8372.018090,
    8869.844191,
    9397.272573,
    9956.063479,
    10548.081821,
    11175.303406,
    11839.821527,
    12543.853951,
];

#[inline(always)]
pub fn midi_to_hz(note: f32) -> f32 {
    let idx = note as usize;
    if note >= 0.0 && (idx as f32 == note) && idx < 128 {
        MIDI_PITCH_TABLE[idx]
    } else {
        440.0 * 2f32.powf((note - 69.0) / 12.0)
    }
}

/// Phase-accumulator oscillator with PolyBLEP anti-aliasing for saw/square.
#[derive(Clone, Default)]
pub struct Osc {
    phase: f32,
}
impl Osc {
    #[inline(always)]
    fn polyblep(t: f32, dt: f32) -> f32 {
        if t < dt {
            let x = t / dt;
            x + x - x * x - 1.0
        } else if t > 1.0 - dt {
            let x = (t - 1.0) / dt;
            x * x + x + x + 1.0
        } else {
            0.0
        }
    }
    #[inline(always)]
    pub fn sine(&mut self, hz: f32, sr: f32) -> f32 {
        let v = (self.phase * TAU).sin();
        self.advance(hz, sr);
        v
    }
    #[inline(always)]
    pub fn saw(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = hz / sr;
        let v = 2.0 * self.phase - 1.0 - Self::polyblep(self.phase, dt);
        self.advance(hz, sr);
        v
    }
    #[inline(always)]
    pub fn square(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = hz / sr;
        let mut v = if self.phase < 0.5 { 1.0 } else { -1.0 };
        v += Self::polyblep(self.phase, dt);
        v -= Self::polyblep((self.phase + 0.5) % 1.0, dt);
        self.advance(hz, sr);
        v
    }
    #[inline(always)]
    pub fn triangle(&mut self, hz: f32, sr: f32) -> f32 {
        let v = 4.0 * (self.phase - 0.5).abs() - 1.0;
        self.advance(hz, sr);
        v
    }
    /// Glottal-like pulse: band-rich source used for voice synthesis.
    #[inline(always)]
    pub fn glottal(&mut self, hz: f32, sr: f32) -> f32 {
        let p = self.phase;
        let v = if p < 0.6 {
            (p / 0.6 * std::f32::consts::PI).sin()
        } else {
            -0.3 * ((p - 0.6) / 0.4 * std::f32::consts::PI).sin()
        };
        self.advance(hz, sr);
        v
    }
    #[inline(always)]
    fn advance(&mut self, hz: f32, sr: f32) {
        self.phase += hz / sr;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }
}

/// Linear ADSR; call `gate_off` to release.
#[derive(Clone)]
pub struct Adsr {
    a: f32,
    d: f32,
    s: f32,
    r: f32,
    level: f32,
    stage: u8,
    sr: f32,
    /// Per-sample release decrement, fixed at `gate_off` from the level at that moment.
    rel_step: f32,
}
impl Adsr {
    pub fn new(a: f32, d: f32, s: f32, r: f32, sr: f32) -> Self {
        Self {
            a,
            d,
            s,
            r,
            level: 0.0,
            stage: 0,
            sr,
            rel_step: 0.0,
        }
    }
    #[inline(always)]
    pub fn gate_off(&mut self) {
        if self.stage < 3 {
            self.stage = 3;
            // Fall from the *current* level to zero in `r` seconds, regardless of
            // the sustain level (a sustain of 0, e.g. plucks, must still release).
            self.rel_step = self.level / (self.r * self.sr).max(1.0);
        }
    }
    #[inline(always)]
    pub fn next(&mut self) -> f32 {
        match self.stage {
            0 => {
                self.level += 1.0 / (self.a * self.sr).max(1.0);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = 1;
                }
            }
            1 => {
                self.level -= (1.0 - self.s) / (self.d * self.sr).max(1.0);
                if self.level <= self.s {
                    self.level = self.s;
                    self.stage = 2;
                }
            }
            2 => {}
            _ => {
                self.level -= self.rel_step;
                if self.level < 0.0 {
                    self.level = 0.0;
                }
            }
        }
        self.level
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsrStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Analog exponential ADSR envelope with punchy attack and smooth musical release.
#[derive(Clone, Debug)]
pub struct AnalogAdsr {
    attack_coef: f32,
    decay_coef: f32,
    sustain_lvl: f32,
    release_coef: f32,
    stage: AdsrStage,
    level: f32,
}

impl AnalogAdsr {
    pub fn new(attack_s: f32, decay_s: f32, sustain: f32, release_s: f32, sr: f32) -> Self {
        let sr = sr.max(1.0);
        let attack_s = attack_s.max(0.0005);
        let decay_s = decay_s.max(0.001);
        let release_s = release_s.max(0.001);
        let sustain = sustain.clamp(0.0, 1.0);

        let attack_coef = 1.0 - (-1.0 / (attack_s * sr * 0.35)).exp();
        let decay_coef = 1.0 - (-1.0 / (decay_s * sr * 0.3)).exp();
        let release_coef = 1.0 - (-1.0 / (release_s * sr * 0.3)).exp();

        Self {
            attack_coef,
            decay_coef,
            sustain_lvl: sustain,
            release_coef,
            stage: AdsrStage::Attack,
            level: 0.0,
        }
    }

    #[inline(always)]
    pub fn gate_off(&mut self) {
        if self.stage != AdsrStage::Idle {
            self.stage = AdsrStage::Release;
        }
    }

    #[inline(always)]
    pub fn is_active(&self) -> bool {
        self.stage != AdsrStage::Idle
    }

    #[inline(always)]
    pub fn next(&mut self) -> f32 {
        match self.stage {
            AdsrStage::Idle => 0.0,
            AdsrStage::Attack => {
                // Target slightly above 1.0 (1.2) for fast punchy analog rise
                self.level += self.attack_coef * (1.2 - self.level);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
                self.level
            }
            AdsrStage::Decay => {
                self.level += self.decay_coef * (self.sustain_lvl - self.level);
                if (self.level - self.sustain_lvl).abs() < 1e-4 {
                    self.level = self.sustain_lvl;
                    self.stage = AdsrStage::Sustain;
                }
                self.level
            }
            AdsrStage::Sustain => self.level,
            AdsrStage::Release => {
                self.level += self.release_coef * (0.0 - self.level);
                if self.level < 1e-3 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
                self.level
            }
        }
    }
}

/// Resonant biquad filter (RBJ Audio EQ Cookbook).
#[derive(Clone, Debug, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn raw(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn bandpass(freq: f32, q: f32, sr: f32) -> Self {
        let mut b = Self::default();
        b.set_bandpass(freq, q, sr);
        b
    }
    pub fn set_bandpass(&mut self, freq: f32, q: f32, sr: f32) {
        let w = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b1 = 0.0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }
    pub fn lowpass(freq: f32, q: f32, sr: f32) -> Self {
        let w = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        let c = w.cos();
        Self {
            b0: (1.0 - c) / 2.0 / a0,
            b1: (1.0 - c) / a0,
            b2: (1.0 - c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn highpass(freq: f32, q: f32, sr: f32) -> Self {
        let w = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        let c = w.cos();
        Self {
            b0: (1.0 + c) / 2.0 / a0,
            b1: -(1.0 + c) / a0,
            b2: (1.0 + c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn low_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * 2.0f32.sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        Self {
            b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0,
            b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0,
            a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn high_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * 2.0f32.sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn peaking(freq: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * freq.clamp(20.0, sr * 0.45) / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));

        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 3-band parametric EQ (Low Shelf, Peaking Mid, High Shelf).
#[derive(Clone)]
pub struct Eq3Band {
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}
impl Eq3Band {
    pub fn new(
        low_gain: f32,
        mid_gain: f32,
        mid_freq: f32,
        mid_q: f32,
        high_gain: f32,
        sr: f32,
    ) -> Self {
        Self {
            low: Biquad::low_shelf(120.0, low_gain, sr),
            mid: Biquad::peaking(mid_freq.clamp(100.0, 10000.0), mid_q.max(0.2), mid_gain, sr),
            high: Biquad::high_shelf(6500.0, high_gain, sr),
        }
    }
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.high.process(self.mid.process(self.low.process(x)))
    }
}

/// Filter mode for the zero-delay feedback State-Variable Filter (SVF).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SvfMode {
    #[default]
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
}

/// Zero-delay feedback State Variable Filter (TPT SVF, Andrew Simper).
/// Unconditionally stable with clean frequency response from 20Hz to Nyquist.
#[derive(Clone, Debug, Default)]
pub struct StateVariableFilter {
    s1: f32,
    s2: f32,
}

impl StateVariableFilter {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn process(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        mode: SvfMode,
        sr: f32,
    ) -> f32 {
        let g = (std::f32::consts::PI * cutoff_hz.clamp(10.0, sr * 0.48) / sr).tan();
        let q = 0.5 / (1.0 - resonance.clamp(0.0, 0.98) * 0.98).max(0.01);
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.s2;
        let v1 = a1 * self.s1 + a2 * v3;
        let v2 = self.s2 + a2 * self.s1 + a3 * v3;
        self.s1 = 2.0 * v1 - self.s1;
        self.s2 = 2.0 * v2 - self.s2;
        match mode {
            SvfMode::Lowpass => v2,
            SvfMode::Bandpass => v1,
            SvfMode::Highpass => input - k * v1 - v2,
            SvfMode::Notch => input - k * v1,
        }
    }
}

/// 4-pole (24dB/octave) Moog-style resonant ladder filter with non-linear saturation.
#[derive(Clone, Debug, Default)]
pub struct MoogLadder {
    s: [f32; 4],
}

impl MoogLadder {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32, sr: f32) -> f32 {
        let wc = (TAU * cutoff_hz.clamp(10.0, sr * 0.45) / sr).min(1.5);
        let f = (wc * 0.5).tan().clamp(0.0001, 0.999);
        let res = resonance.clamp(0.0, 0.98) * 3.98;

        // Feedback with soft tanh saturation for classic warm analog warmth
        let fb = self.s[3];
        let u = (input - res * fb).tanh();

        self.s[0] += f * (u - self.s[0]);
        self.s[1] += f * (self.s[0] - self.s[1]);
        self.s[2] += f * (self.s[1] - self.s[2]);
        self.s[3] += f * (self.s[2] - self.s[3]);

        // Makeup gain to compensate for passband loss when resonance is high
        let makeup = 1.0 + 0.5 * res;
        self.s[3] * makeup
    }
}

/// 2-stage K-weighting filter per ITU-R BS.1770-4 for LUFS measurement.
/// Stage 1: High shelf filter simulating head acoustic effect (+4dB boost @ 1.5kHz).
/// Stage 2: High-pass RLB weighting filter (~100Hz 2nd order HPF).
#[derive(Clone, Debug)]
pub struct KWeightingFilter {
    pre: Biquad,
    rlb: Biquad,
}

impl KWeightingFilter {
    pub fn new(sr: f32) -> Self {
        let sr = sr.max(8000.0);
        // Stage 1: High shelf
        let f0_pre = 1681.97445;
        let g_db_pre = 3.99984385;
        let q_pre = 0.707175237;

        let k_pre = (std::f32::consts::PI * f0_pre / sr).tan();
        let vh = 10f32.powf(g_db_pre / 20.0);
        let a0_pre = 1.0 + k_pre / q_pre + k_pre * k_pre;
        let b0_pre = (vh + (vh.sqrt() * k_pre) / q_pre + k_pre * k_pre) / a0_pre;
        let b1_pre = (2.0 * (k_pre * k_pre - vh)) / a0_pre;
        let b2_pre = (vh - (vh.sqrt() * k_pre) / q_pre + k_pre * k_pre) / a0_pre;
        let a1_pre = (2.0 * (k_pre * k_pre - 1.0)) / a0_pre;
        let a2_pre = (1.0 - k_pre / q_pre + k_pre * k_pre) / a0_pre;

        let pre = Biquad::raw(b0_pre, b1_pre, b2_pre, a1_pre, a2_pre);

        // Stage 2: High-pass RLB weighting
        let f0_rlb = 38.13547;
        let q_rlb = 0.500327037;

        let k_rlb = (std::f32::consts::PI * f0_rlb / sr).tan();
        let a0_rlb = 1.0 + k_rlb / q_rlb + k_rlb * k_rlb;
        let b0_rlb = 1.0 / a0_rlb;
        let b1_rlb = -2.0 / a0_rlb;
        let b2_rlb = 1.0 / a0_rlb;
        let a1_rlb = (2.0 * (k_rlb * k_rlb - 1.0)) / a0_rlb;
        let a2_rlb = (1.0 - k_rlb / q_rlb + k_rlb * k_rlb) / a0_rlb;

        let rlb = Biquad::raw(b0_rlb, b1_rlb, b2_rlb, a1_rlb, a2_rlb);

        Self { pre, rlb }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.rlb.process(self.pre.process(x))
    }
}

/// 6-band audio filterbank for frequency spectrum & masking analysis.
/// Bands: [SubBass (<60Hz), Bass (60-250Hz), LowMid (250-500Hz), Mid (500-2kHz), HighMid (2-6kHz), High (>6kHz)]
#[derive(Clone, Debug)]
pub struct Filterbank6 {
    b0: Biquad, // Lowpass 60Hz
    b1: Biquad, // Bandpass 60-250Hz (center 122Hz, Q 0.7)
    b2: Biquad, // Bandpass 250-500Hz (center 353Hz, Q 1.4)
    b3: Biquad, // Bandpass 500-2000Hz (center 1000Hz, Q 0.67)
    b4: Biquad, // Bandpass 2000-6000Hz (center 3464Hz, Q 0.86)
    b5: Biquad, // Highpass 6000Hz
}

impl Filterbank6 {
    pub fn new(sr: f32) -> Self {
        Self {
            b0: Biquad::lowpass(60.0, 0.707, sr),
            b1: Biquad::bandpass(122.0, 0.7, sr),
            b2: Biquad::bandpass(353.0, 1.4, sr),
            b3: Biquad::bandpass(1000.0, 0.67, sr),
            b4: Biquad::bandpass(3464.0, 0.86, sr),
            b5: Biquad::highpass(6000.0, 0.707, sr),
        }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> [f32; 6] {
        [
            self.b0.process(x),
            self.b1.process(x),
            self.b2.process(x),
            self.b3.process(x),
            self.b4.process(x),
            self.b5.process(x),
        ]
    }
}

/// Stereo Delay effect with feedback, damping, and optional ping-pong.
#[derive(Clone)]
pub struct StereoDelay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    idx_l: usize,
    idx_r: usize,
    delay_samples_l: usize,
    delay_samples_r: usize,
    feedback: f32,
    mix: f32,
    ping_pong: bool,
    damp_l: f32,
    damp_r: f32,
}
impl StereoDelay {
    pub fn new(time_ms: f32, feedback: f32, mix: f32, ping_pong: bool, sr: f32) -> Self {
        let max_samples = (2.0 * sr) as usize; // up to 2 seconds
        let delay_samples = ((time_ms / 1000.0) * sr).clamp(1.0, max_samples as f32 - 1.0) as usize;
        let delay_r = if ping_pong {
            (delay_samples * 3 / 2).min(max_samples - 1)
        } else {
            delay_samples
        };
        Self {
            buf_l: vec![0.0; max_samples],
            buf_r: vec![0.0; max_samples],
            idx_l: 0,
            idx_r: 0,
            delay_samples_l: delay_samples,
            delay_samples_r: delay_r,
            feedback: feedback.clamp(0.0, 0.95),
            mix: mix.clamp(0.0, 1.0),
            ping_pong,
            damp_l: 0.0,
            damp_r: 0.0,
        }
    }
    #[inline(always)]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let read_l = (self.idx_l + self.buf_l.len() - self.delay_samples_l) % self.buf_l.len();
        let read_r = (self.idx_r + self.buf_r.len() - self.delay_samples_r) % self.buf_r.len();

        let delayed_l = self.buf_l[read_l];
        let delayed_r = self.buf_r[read_r];

        // 6dB/oct high damping on feedback
        self.damp_l = self.damp_l * 0.4 + delayed_l * 0.6;
        self.damp_r = self.damp_r * 0.4 + delayed_r * 0.6;

        let fb_l = if self.ping_pong {
            self.damp_r * self.feedback
        } else {
            self.damp_l * self.feedback
        };
        let fb_r = if self.ping_pong {
            self.damp_l * self.feedback
        } else {
            self.damp_r * self.feedback
        };

        self.buf_l[self.idx_l] = in_l + fb_l;
        self.buf_r[self.idx_r] = in_r + fb_r;

        self.idx_l = (self.idx_l + 1) % self.buf_l.len();
        self.idx_r = (self.idx_r + 1) % self.buf_r.len();

        let out_l = in_l * (1.0 - self.mix) + delayed_l * self.mix;
        let out_r = in_r * (1.0 - self.mix) + delayed_r * self.mix;
        (out_l, out_r)
    }
}

/// Comb filter used in algorithmic reverberation.
#[derive(Clone)]
struct CombFilter {
    buffer: Vec<f32>,
    idx: usize,
    filter_store: f32,
    damp1: f32,
    damp2: f32,
    feedback: f32,
}
impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            idx: 0,
            filter_store: 0.0,
            damp1: 0.5,
            damp2: 0.5,
            feedback: 0.8,
        }
    }
    fn set_damp(&mut self, damp: f32) {
        self.damp1 = damp;
        self.damp2 = 1.0 - damp;
    }
    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.idx];
        self.filter_store = output * self.damp2 + self.filter_store * self.damp1;
        self.buffer[self.idx] = input + self.filter_store * self.feedback;
        self.idx = (self.idx + 1) % self.buffer.len();
        output
    }
}

/// Allpass filter used in algorithmic reverberation.
#[derive(Clone)]
struct AllpassFilter {
    buffer: Vec<f32>,
    idx: usize,
    feedback: f32,
}
impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            idx: 0,
            feedback: 0.5,
        }
    }
    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let buf_out = self.buffer[self.idx];
        let output = -input + buf_out;
        self.buffer[self.idx] = input + buf_out * self.feedback;
        self.idx = (self.idx + 1) % self.buffer.len();
        output
    }
}

/// Freeverb-style stereo algorithmic reverberator.
#[derive(Clone)]
pub struct StereoReverb {
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    allpasses_l: Vec<AllpassFilter>,
    allpasses_r: Vec<AllpassFilter>,
    gain: f32,
    room_size: f32,
    damping: f32,
    width: f32,
    mix: f32,
}
impl StereoReverb {
    pub fn new(room_size: f32, damping: f32, width: f32, mix: f32, sr: f32) -> Self {
        let scale = sr / 44100.0;
        let comb_tunings = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_tunings = [556, 441, 341, 225];
        let stereo_spread = 23;

        let combs_l = comb_tunings
            .iter()
            .map(|&t| CombFilter::new((t as f32 * scale) as usize))
            .collect();
        let combs_r = comb_tunings
            .iter()
            .map(|&t| CombFilter::new(((t + stereo_spread) as f32 * scale) as usize))
            .collect();
        let allpasses_l = allpass_tunings
            .iter()
            .map(|&t| AllpassFilter::new((t as f32 * scale) as usize))
            .collect();
        let allpasses_r = allpass_tunings
            .iter()
            .map(|&t| AllpassFilter::new(((t + stereo_spread) as f32 * scale) as usize))
            .collect();

        let mut rev = Self {
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
            gain: 0.015,
            room_size: room_size.clamp(0.0, 1.0),
            damping: damping.clamp(0.0, 1.0),
            width: width.clamp(0.0, 1.0),
            mix: mix.clamp(0.0, 1.0),
        };
        rev.update_params();
        rev
    }
    fn update_params(&mut self) {
        let fb = 0.7 + self.room_size * 0.28;
        let damp = self.damping * 0.4;
        for c in &mut self.combs_l {
            c.feedback = fb;
            c.set_damp(damp);
        }
        for c in &mut self.combs_r {
            c.feedback = fb;
            c.set_damp(damp);
        }
    }
    #[inline(always)]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let input = (in_l + in_r) * self.gain;
        let mut out_l = 0.0;
        let mut out_r = 0.0;

        for c in &mut self.combs_l {
            out_l += c.process(input);
        }
        for c in &mut self.combs_r {
            out_r += c.process(input);
        }

        for a in &mut self.allpasses_l {
            out_l = a.process(out_l);
        }
        for a in &mut self.allpasses_r {
            out_r = a.process(out_r);
        }

        let wet1 = 0.5 * (1.0 + self.width);
        let wet2 = 0.5 * (1.0 - self.width);

        let rev_l = out_l * wet1 + out_r * wet2;
        let rev_r = out_r * wet1 + out_l * wet2;

        let final_l = in_l * (1.0 - self.mix) + rev_l * self.mix;
        let final_r = in_r * (1.0 - self.mix) + rev_r * self.mix;
        (final_l, final_r)
    }
}

/// Dynamic range compressor.
#[derive(Clone)]
pub struct Compressor {
    threshold_db: f32,
    ratio: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_gain: f32,
    envelope: f32,
}
impl Compressor {
    pub fn new(
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
        sr: f32,
    ) -> Self {
        Self {
            threshold_db,
            ratio: ratio.max(1.0),
            attack_coeff: (-1.0 / ((attack_ms * 0.001 * sr).max(1.0))).exp(),
            release_coeff: (-1.0 / ((release_ms * 0.001 * sr).max(1.0))).exp(),
            makeup_gain: 10f32.powf(makeup_db / 20.0),
            envelope: 0.0,
        }
    }
    #[inline(always)]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let max_val = in_l.abs().max(in_r.abs());
        let coeff = if max_val > self.envelope {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.envelope = max_val + (self.envelope - max_val) * coeff;

        let env_db = if self.envelope > 1e-6 {
            20.0 * self.envelope.log10()
        } else {
            -120.0
        };

        let gain_db = if env_db > self.threshold_db {
            (self.threshold_db - env_db) * (1.0 - 1.0 / self.ratio)
        } else {
            0.0
        };

        let gain = 10f32.powf(gain_db / 20.0) * self.makeup_gain;
        (in_l * gain, in_r * gain)
    }
}

/// xorshift white noise (deterministic, no deps).
#[derive(Clone)]
pub struct Noise(u32);
impl Noise {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adsr_release_reaches_zero_even_with_zero_sustain() {
        let sr = 44100.0;
        // Pluck-style envelope released while still decaying (note shorter than decay).
        let mut env = Adsr::new(0.002, 0.4, 0.0, 0.15, sr);
        for _ in 0..(0.1 * sr) as usize {
            env.next();
        }
        let at_gate_off = env.next();
        assert!(
            at_gate_off > 0.5,
            "still decaying at gate-off: {at_gate_off}"
        );
        env.gate_off();
        // after the release time (+ margin) the level must be exactly zero
        let mut last = at_gate_off;
        for _ in 0..(0.16 * sr) as usize {
            let v = env.next();
            assert!(v <= last + 1e-6, "release must be monotonic");
            last = v;
        }
        assert_eq!(last, 0.0);
    }
    #[test]
    fn adsr_release_from_sustain_unchanged() {
        let sr = 44100.0;
        let mut env = Adsr::new(0.01, 0.1, 0.7, 0.25, sr);
        for _ in 0..(0.5 * sr) as usize {
            env.next();
        }
        env.gate_off();
        let mut n = 0usize;
        while env.next() > 0.0 {
            n += 1;
        }
        let secs = n as f32 / sr;
        assert!((secs - 0.25).abs() < 0.01, "release took {secs}s");
    }
    #[test]
    fn a4_is_440() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
    }
    #[test]
    fn osc_bounded() {
        let mut o = Osc::default();
        for _ in 0..1000 {
            assert!(o.saw(440.0, 44100.0).abs() <= 1.5);
        }
    }
    #[test]
    fn eq_processing() {
        let mut eq = Eq3Band::new(3.0, -2.0, 1000.0, 1.0, 4.0, 44100.0);
        let out = eq.process(0.5);
        assert!(out.is_finite());
    }
    #[test]
    fn delay_reverb_smoke() {
        let mut delay = StereoDelay::new(100.0, 0.5, 0.4, true, 44100.0);
        let (dl, dr) = delay.process(0.8, 0.8);
        assert!(dl.is_finite() && dr.is_finite());

        let mut rev = StereoReverb::new(0.8, 0.3, 1.0, 0.35, 44100.0);
        let (rl, rr) = rev.process(0.8, 0.8);
        assert!(rl.is_finite() && rr.is_finite());
    }

    #[test]
    fn analog_adsr_behavior() {
        let mut env = AnalogAdsr::new(0.01, 0.05, 0.5, 0.05, 44100.0);
        let mut max_val = 0.0f32;
        for _ in 0..1000 {
            let v = env.next();
            assert!((0.0..=1.2).contains(&v) && v.is_finite());
            if v > max_val {
                max_val = v;
            }
        }
        assert!(max_val >= 0.95);
        env.gate_off();
        for _ in 0..10000 {
            let v = env.next();
            assert!(v >= 0.0 && v.is_finite());
        }
        assert_eq!(env.next(), 0.0);
        assert!(!env.is_active());
    }

    #[test]
    fn moog_ladder_stability() {
        let mut moog = MoogLadder::new();
        for i in 0..10000 {
            let x = ((i as f32 * 0.1).sin()).clamp(-1.0, 1.0);
            let y = moog.process(x, 1000.0, 0.8, 44100.0);
            assert!(
                y.is_finite(),
                "Moog ladder produced non-finite output at sample {}",
                i
            );
            assert!(y.abs() <= 2.0, "Moog ladder blew up: {}", y);
        }
    }

    #[test]
    fn svf_modes_stability() {
        let mut svf = StateVariableFilter::new();
        for mode in [
            SvfMode::Lowpass,
            SvfMode::Highpass,
            SvfMode::Bandpass,
            SvfMode::Notch,
        ] {
            for i in 0..1000 {
                let x = if i % 2 == 0 { 0.5 } else { -0.5 };
                let y = svf.process(x, 800.0, 0.7, mode, 44100.0);
                assert!(y.is_finite());
            }
        }
    }

    #[test]
    fn k_weighting_filter_smoke() {
        let mut kw = KWeightingFilter::new(48000.0);
        for i in 0..1000 {
            let x = (i as f32 * 0.05).sin();
            let y = kw.process(x);
            assert!(y.is_finite());
        }
    }

    #[test]
    fn filterbank6_smoke() {
        let mut fb = Filterbank6::new(44100.0);
        let bands = fb.process(0.5);
        for b in bands {
            assert!(b.is_finite());
        }
    }
}
