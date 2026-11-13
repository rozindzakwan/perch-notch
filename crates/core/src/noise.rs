//! White-noise mixer model and the procedural sound generator.
//!
//! Every sound is synthesised from seeded noise and a few filters, so Perch
//! ships no audio files at all and the generator is pure math that unit tests
//! can check (level, bounds, determinism). The audio backend (rodio) only pulls
//! samples out of [`NoiseGenerator`]; it is opened while at least one layer is
//! audible and dropped as soon as the mix goes silent.

/// The sounds Perch can generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoiseKind {
    /// Steady rain: pink-ish noise plus sparse droplets.
    Rain,
    /// Forest: soft brown-noise wind plus occasional synthesised chirps.
    Forest,
    /// Waves: brown noise swelling on a slow LFO.
    Waves,
}

impl NoiseKind {
    /// Every kind, in UI order.
    pub const ALL: &'static [NoiseKind] = &[NoiseKind::Rain, NoiseKind::Forest, NoiseKind::Waves];

    /// Display label.
    pub fn label(&self) -> &'static str {
        match self {
            NoiseKind::Rain => "Rain",
            NoiseKind::Forest => "Forest",
            NoiseKind::Waves => "Waves",
        }
    }

    fn index(&self) -> usize {
        match self {
            NoiseKind::Rain => 0,
            NoiseKind::Forest => 1,
            NoiseKind::Waves => 2,
        }
    }
}

/// Which layers are on and how loud. Levels are 0.0 ..= 1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseMix {
    on: [bool; 3],
    level: [f32; 3],
    /// Master volume.
    pub master: f32,
}

impl Default for NoiseMix {
    fn default() -> Self {
        Self {
            on: [false; 3],
            level: [0.6; 3],
            master: 0.7,
        }
    }
}

impl NoiseMix {
    /// Whether a layer is on.
    pub fn is_on(&self, kind: NoiseKind) -> bool {
        self.on[kind.index()]
    }

    /// A layer's level.
    pub fn level(&self, kind: NoiseKind) -> f32 {
        self.level[kind.index()]
    }

    /// Toggle a layer.
    pub fn toggle(&mut self, kind: NoiseKind) {
        let i = kind.index();
        self.on[i] = !self.on[i];
    }

    /// Set a layer's on state.
    pub fn set_on(&mut self, kind: NoiseKind, on: bool) {
        self.on[kind.index()] = on;
    }

    /// Set a layer's level, clamped.
    pub fn set_level(&mut self, kind: NoiseKind, level: f32) {
        self.level[kind.index()] = clamp01(level);
    }

    /// Whether anything is audible; the audio device is only open while true.
    pub fn is_audible(&self) -> bool {
        self.master > 0.0
            && NoiseKind::ALL
                .iter()
                .any(|k| self.is_on(*k) && self.level(*k) > 0.0)
    }

    /// Effective gain of one layer (0 when off).
    pub fn gain(&self, kind: NoiseKind) -> f32 {
        if self.is_on(kind) {
            clamp01(self.level(kind)) * clamp01(self.master)
        } else {
            0.0
        }
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// xorshift32: tiny, fast, deterministic, good enough for audio noise.
#[derive(Debug, Clone)]
struct Rng(u32);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in [-1, 1).
    fn bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Mono sample generator. Call [`NoiseGenerator::set_mix`] whenever the user
/// changes something; gains glide to the new values to avoid clicks.
#[derive(Debug, Clone)]
pub struct NoiseGenerator {
    rate: f32,
    rng: Rng,
    mix: NoiseMix,
    gains: [f32; 3],
    // Pink noise (Paul Kellet's economy filter).
    pink: [f32; 3],
    // Brown noise integrator.
    brown: f32,
    brown2: f32,
    // Droplets.
    drop_env: f32,
    drop_tone: f32,
    // Chirps.
    chirp_env: f32,
    chirp_phase: f32,
    chirp_freq: f32,
    // Wave LFO phase.
    lfo: f32,
}

impl NoiseGenerator {
    /// A generator at `sample_rate` Hz with a fixed seed.
    pub fn new(sample_rate: u32, seed: u32) -> Self {
        Self {
            rate: sample_rate.max(8000) as f32,
            rng: Rng(seed | 1),
            mix: NoiseMix::default(),
            gains: [0.0; 3],
            pink: [0.0; 3],
            brown: 0.0,
            brown2: 0.0,
            drop_env: 0.0,
            drop_tone: 0.0,
            chirp_env: 0.0,
            chirp_phase: 0.0,
            chirp_freq: 3000.0,
            lfo: 0.0,
        }
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.rate as u32
    }

    /// Update the target mix.
    pub fn set_mix(&mut self, mix: NoiseMix) {
        self.mix = mix;
    }

    /// The target mix.
    pub fn mix(&self) -> NoiseMix {
        self.mix
    }

    /// Produce one sample in [-1, 1].
    pub fn next_sample(&mut self) -> f32 {
        // Glide gains (~20 ms) so toggles never click.
        let glide = 1.0 / (0.02 * self.rate);
        for k in NoiseKind::ALL {
            let i = k.index();
            let target = self.mix.gain(*k);
            let g = &mut self.gains[i];
            *g += (target - *g) * glide.min(1.0);
        }

        let white = self.rng.bipolar();

        // Pink.
        self.pink[0] = 0.99765 * self.pink[0] + white * 0.099_046;
        self.pink[1] = 0.963 * self.pink[1] + white * 0.296_516_4;
        self.pink[2] = 0.57 * self.pink[2] + white * 1.052_691_3;
        let pink = (self.pink[0] + self.pink[1] + self.pink[2] + white * 0.1848) * 0.2;

        // Brown (leaky integrator), and a darker second stage for waves.
        self.brown = (self.brown + white * 0.02) * 0.998;
        self.brown2 = (self.brown2 + self.brown * 0.05) * 0.999;
        let brown = self.brown * 3.0;

        let mut out = 0.0;

        // Rain: pink bed + droplets (short decaying tone bursts).
        if self.gains[0] > 1e-4 {
            if self.rng.unit() < 40.0 / self.rate {
                self.drop_env = 0.6 + self.rng.unit() * 0.4;
                self.drop_tone = 0.0;
            }
            self.drop_env *= 1.0 - 60.0 / self.rate;
            self.drop_tone += 2.0 * std::f32::consts::PI * 1800.0 / self.rate;
            let drop = self.drop_env * self.drop_tone.sin() * 0.15;
            out += (pink * 0.9 + drop) * self.gains[0];
        }

        // Forest: wind (brown) + occasional chirps sweeping upwards.
        if self.gains[1] > 1e-4 {
            if self.chirp_env < 0.01 && self.rng.unit() < 0.4 / self.rate {
                self.chirp_env = 1.0;
                self.chirp_freq = 2500.0 + self.rng.unit() * 2000.0;
            }
            self.chirp_env *= 1.0 - 12.0 / self.rate;
            self.chirp_freq += 4000.0 / self.rate;
            self.chirp_phase += 2.0 * std::f32::consts::PI * self.chirp_freq / self.rate;
            if self.chirp_phase > std::f32::consts::TAU {
                self.chirp_phase -= std::f32::consts::TAU;
            }
            let chirp = self.chirp_env * self.chirp_phase.sin() * 0.12;
            out += (brown * 0.5 + chirp) * self.gains[1];
        }

        // Waves: dark brown noise swelling on an ~8 s LFO.
        if self.gains[2] > 1e-4 {
            self.lfo += std::f32::consts::TAU / (8.0 * self.rate);
            if self.lfo > std::f32::consts::TAU {
                self.lfo -= std::f32::consts::TAU;
            }
            let swell = 0.25 + 0.75 * (0.5 + 0.5 * self.lfo.sin()).powi(2);
            out += (self.brown2 * 6.0 + brown * 0.3) * swell * self.gains[2];
        }

        if out.is_finite() {
            out.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }
}
