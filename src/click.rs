use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn synth_click() -> Vec<f32> {
    // 793 samples at 44.1 kHz: PCG noise + 2650/5300 Hz sine, 8% attack, exp(-10.5 * progress)
    let sample_rate = 44_100.0f64;
    let n = (sample_rate * 0.018) as usize;
    let a = 16650.441064025905f64;
    let b = 33300.88212805181f64;
    let mut seed: u64 = 0x9E37_79B9;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / sample_rate;
        let x = i as f64 / (n - 1) as f64;
        let env = ((x / 0.08).min(1.0) * (-10.5 * x).exp()) as f32;
        seed = seed.wrapping_mul(0x5851_F42D_4C95_7F2D).wrapping_add(1);
        let noise = ((((seed >> 33) & 0xFFFF) as f64 / 32768.0) - 1.0) as f32;
        let s1 = ((t * a).sin() as f32) * 0.34f32;
        let s2 = ((t * b).sin() as f32) * 0.18f32;
        let s3 = noise * 0.18f32;
        out.push(((s1 + s2) + s3) * env * 0.34f32);
    }
    out
}

struct Voice {
    pos: f32,
}

pub struct Clicker {
    voices: Arc<Mutex<Vec<Voice>>>,
    _stream: cpal::Stream,
}

impl Clicker {
    pub fn new(gain: f32) -> Option<Self> {
        let source_rate = 44_100.0f32;
        let mut mono = synth_click();
        if mono.is_empty() {
            return None;
        }
        // normalise to the same loudness the recorded reference had, then the user volume applies
        let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        let norm = 0.85 / peak;
        for s in &mut mono {
            *s *= norm;
        }
        let samples = Arc::new(mono);
        let voices: Arc<Mutex<Vec<Voice>>> = Arc::new(Mutex::new(Vec::new()));

        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = device.default_output_config().ok()?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;
        let step = source_rate / config.sample_rate.0 as f32;

        let err_fn = |e| eprintln!("tera audio: {e}");
        let (v, s) = (voices.clone(), samples.clone());
        let stream = match sample_format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &config,
                    move |data: &mut [f32], _| mix_f32(data, &v, &s, channels, step, gain),
                    err_fn,
                    None,
                )
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    &config,
                    move |data: &mut [i16], _| mix_i16(data, &v, &s, channels, step, gain),
                    err_fn,
                    None,
                )
                .ok()?,
            cpal::SampleFormat::U16 => device
                .build_output_stream(
                    &config,
                    move |data: &mut [u16], _| mix_u16(data, &v, &s, channels, step, gain),
                    err_fn,
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;
        Some(Self {
            voices,
            _stream: stream,
        })
    }

    pub fn play(&self) {
        if let Ok(mut v) = self.voices.lock() {
            if v.len() < 8 {
                v.push(Voice { pos: 0.0 });
            }
        }
    }
}

fn next_voice(voice: &mut Voice, samples: &[f32], step: f32) -> f32 {
    let i = voice.pos as usize;
    if i >= samples.len() {
        return 0.0;
    }
    let frac = voice.pos - i as f32;
    let s = if i + 1 < samples.len() {
        samples[i] * (1.0 - frac) + samples[i + 1] * frac
    } else {
        samples[i]
    };
    voice.pos += step;
    s
}

fn mix_f32(
    data: &mut [f32],
    voices: &Mutex<Vec<Voice>>,
    samples: &[f32],
    channels: usize,
    step: f32,
    gain: f32,
) {
    let mut vs = voices.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let mut v = 0.0;
        for voice in vs.iter_mut() {
            v += next_voice(voice, samples, step);
        }
        let out = (v * gain).clamp(-1.0, 1.0);
        for ch in frame {
            *ch = out;
        }
    }
    vs.retain(|v| (v.pos as usize) < samples.len());
}

fn mix_i16(
    data: &mut [i16],
    voices: &Mutex<Vec<Voice>>,
    samples: &[f32],
    channels: usize,
    step: f32,
    gain: f32,
) {
    let mut vs = voices.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let mut v = 0.0;
        for voice in vs.iter_mut() {
            v += next_voice(voice, samples, step);
        }
        let out = ((v * gain).clamp(-1.0, 1.0) * 32767.0) as i16;
        for ch in frame {
            *ch = out;
        }
    }
    vs.retain(|v| (v.pos as usize) < samples.len());
}

fn mix_u16(
    data: &mut [u16],
    voices: &Mutex<Vec<Voice>>,
    samples: &[f32],
    channels: usize,
    step: f32,
    gain: f32,
) {
    let mut vs = voices.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let mut v = 0.0;
        for voice in vs.iter_mut() {
            v += next_voice(voice, samples, step);
        }
        let out = ((v * gain).clamp(-1.0, 1.0) * 32767.0 + 32768.0) as u16;
        for ch in frame {
            *ch = out;
        }
    }
    vs.retain(|v| (v.pos as usize) < samples.len());
}

#[cfg(test)]
mod tests {
    use super::synth_click;

    #[test]
    fn click_has_reference_shape() {
        let s = synth_click();
        assert_eq!(s.len(), 793);
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.07886).abs() < 1e-4, "peak {peak}");
    }
}
