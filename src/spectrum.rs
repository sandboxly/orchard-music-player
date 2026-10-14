//! The visualizer: logarithmic spectrum bars with falling peak caps
//! (Hann-windowed FFT of the latest output samples), or an oscilloscope.

use std::sync::Arc;
use std::time::Instant;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

pub const FFT_SIZE: usize = 2048;
const MIN_HZ: f64 = 30.0;
const MAX_HZ: f64 = 16_000.0;
const DB_FLOOR: f64 = -62.0;
const DB_CEILING: f64 = -10.0;
/// Music has less energy up high; lift the treble so the bars look even.
const TILT_DB_PER_OCTAVE: f64 = 3.0;
/// Bar fall speed, in full heights per second.
const FALL: f32 = 1.6;
/// Peak caps wait this long, then fall with gravity.
const PEAK_HOLD: f32 = 0.45;
const PEAK_GRAVITY: f32 = 2.2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Bars,
    Scope,
    Off,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Bars => Mode::Scope,
            Mode::Scope => Mode::Off,
            Mode::Off => Mode::Bars,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Bars => "bars",
            Mode::Scope => "scope",
            Mode::Off => "off",
        }
    }

    pub fn from_label(s: &str) -> Self {
        match s {
            "scope" => Mode::Scope,
            "off" => Mode::Off,
            _ => Mode::Bars,
        }
    }
}

pub struct Analyzer {
    plan: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    /// Displayed bar heights 0..1.
    pub bars: Vec<f32>,
    pub peaks: Vec<f32>,
    peak_age: Vec<f32>,
    peak_speed: Vec<f32>,
    last: Option<Instant>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Analyzer::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let plan = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let d = (FFT_SIZE - 1) as f32;
        let window = (0..FFT_SIZE).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / d).cos()).collect();
        Analyzer {
            plan,
            window,
            buf: vec![Complex::default(); FFT_SIZE],
            bars: Vec::new(),
            peaks: Vec::new(),
            peak_age: Vec::new(),
            peak_speed: Vec::new(),
            last: None,
        }
    }

    /// Whether anything still moves (bars falling after a pause).
    pub fn is_animating(&self) -> bool {
        self.bars.iter().chain(&self.peaks).any(|&v| v > 0.001)
    }

    /// Advance the animation: `samples` are the latest [`FFT_SIZE`] mono
    /// samples at `rate` (or none: everything falls), `count` bars wide.
    pub fn update(&mut self, samples: Option<&[f32]>, rate: u32, count: usize, now: Instant) {
        let dt = self.last.map_or(1.0 / 60.0, |l| now.duration_since(l).as_secs_f32()).min(0.1);
        self.last = Some(now);
        if self.bars.len() != count {
            self.bars = vec![0.0; count];
            self.peaks = vec![0.0; count];
            self.peak_age = vec![0.0; count];
            self.peak_speed = vec![0.0; count];
        }
        let target = match samples {
            Some(s) if s.len() >= FFT_SIZE => self.levels(&s[s.len() - FFT_SIZE..], rate, count),
            _ => vec![0.0; count],
        };
        for (i, &t) in target.iter().enumerate() {
            let fallen = (self.bars[i] - FALL * dt).max(0.0);
            self.bars[i] = t.max(fallen);
            if self.bars[i] >= self.peaks[i] {
                self.peaks[i] = self.bars[i];
                self.peak_age[i] = 0.0;
                self.peak_speed[i] = 0.0;
            } else {
                self.peak_age[i] += dt;
                if self.peak_age[i] > PEAK_HOLD {
                    self.peak_speed[i] += PEAK_GRAVITY * dt;
                    self.peaks[i] = (self.peaks[i] - self.peak_speed[i] * dt).max(self.bars[i]);
                }
            }
        }
    }

    /// 0..1 per logarithmic band: the loudest bin in it, in dB between the
    /// floor and the ceiling.
    fn levels(&mut self, samples: &[f32], rate: u32, count: usize) -> Vec<f32> {
        for (i, c) in self.buf.iter_mut().enumerate() {
            *c = Complex::new(samples[i] * self.window[i], 0.0);
        }
        self.plan.process(&mut self.buf);
        let scale = 4.0 / FFT_SIZE as f32;
        let mags: Vec<f32> = self.buf[..=FFT_SIZE / 2].iter().map(|c| c.norm() * scale).collect();
        band_levels(&mags, rate, count)
    }
}

pub fn band_levels(mags: &[f32], rate: u32, count: usize) -> Vec<f32> {
    if count == 0 || mags.len() < 2 || rate == 0 {
        return vec![0.0; count];
    }
    let n = ((mags.len() - 1) * 2) as f64;
    let top = MAX_HZ.min(rate as f64 / 2.0);
    let edge = |i: usize| MIN_HZ * (top / MIN_HZ).powf(i as f64 / count as f64);
    (0..count)
        .map(|b| {
            let (lo_hz, hi_hz) = (edge(b), edge(b + 1));
            let (lo, hi) = (lo_hz * n / rate as f64, hi_hz * n / rate as f64);
            let mag = if hi.floor() > lo.floor() {
                let start = (lo as usize + 1).min(mags.len() - 1);
                let end = (hi as usize + 1).min(mags.len());
                mags[start..end.max(start + 1)].iter().copied().fold(0.0f32, f32::max) as f64
            } else {
                // Narrower than a bin (the bass): interpolate at its centre.
                let c = (lo + hi) / 2.0;
                let i = (c as usize).min(mags.len() - 2);
                let (a, b) = (mags[i] as f64, mags[i + 1] as f64);
                a + (b - a) * (c - i as f64)
            };
            let centre = (lo_hz * hi_hz).sqrt();
            let db = 20.0 * (mag + 1e-12).log10() + TILT_DB_PER_OCTAVE * (centre / 1000.0).log2();
            ((db - DB_FLOOR) / (DB_CEILING - DB_FLOOR)).clamp(0.0, 1.0) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lights_its_band() {
        let rate = 48_000;
        let hz = 1000.0;
        let s: Vec<f32> =
            (0..FFT_SIZE).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect();
        let mut a = Analyzer::new();
        let count = 32;
        a.update(Some(&s), rate, count, Instant::now());
        let loudest = a.bars.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap().0;
        let top = MAX_HZ.min(rate as f64 / 2.0);
        let band = ((hz as f64 / MIN_HZ).ln() / (top / MIN_HZ).ln() * count as f64) as usize;
        assert!(loudest.abs_diff(band) <= 1, "loudest {loudest}, expected {band}");
        assert!(a.bars[loudest] > 0.8);
        assert!(a.bars[0] < 0.2);
    }

    #[test]
    fn silence_falls() {
        let mut a = Analyzer::new();
        let t = Instant::now();
        a.bars = vec![1.0; 4];
        a.peaks = vec![1.0; 4];
        a.peak_age = vec![0.0; 4];
        a.peak_speed = vec![0.0; 4];
        for i in 1..120 {
            a.update(None, 48_000, 4, t + std::time::Duration::from_millis(16 * i));
        }
        assert!(!a.is_animating());
    }
}
