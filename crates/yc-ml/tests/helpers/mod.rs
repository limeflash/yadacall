/// Makes a deterministic, voiced vowel-like signal for local model integration checks.
pub fn render_voiced_signal(seconds: f32, pitch_hz: f32, text_seed: usize) -> Vec<f32> {
    const RATE: usize = 16_000;
    const VOWELS: [[f32; 3]; 5] = [
        [780.0, 1_250.0, 2_650.0],
        [520.0, 1_850.0, 2_650.0],
        [320.0, 2_250.0, 3_000.0],
        [500.0, 950.0, 2_500.0],
        [350.0, 800.0, 2_200.0],
    ];
    let sample_count = (seconds * RATE as f32) as usize;
    let mut noise_state = 0x1234_5678_u32.wrapping_add(text_seed as u32);
    (0..sample_count)
        .map(|index| {
            let time = index as f32 / RATE as f32;
            let syllable_rate = 3.5 + (text_seed % 5) as f32 * 0.12;
            let vowel = VOWELS[((time * 3.8) as usize) % VOWELS.len()];
            let formant_scale = if pitch_hz >= 180.0 {
                1.18
            } else if pitch_hz <= 100.0 {
                0.9
            } else {
                1.0
            };
            let fundamental =
                pitch_hz * (1.0 + 0.018 * (2.0 * std::f32::consts::PI * 5.1 * time).sin());
            let phase = 2.0 * std::f32::consts::PI * fundamental * time;
            let envelope = 0.38
                + 0.62
                    * (2.0 * std::f32::consts::PI * syllable_rate * time)
                        .sin()
                        .max(0.0);
            let mut sample = 0.0;
            for harmonic in 1..=50 {
                let frequency = fundamental * harmonic as f32;
                let formant_gain = vowel
                    .iter()
                    .map(|formant| {
                        let distance = (frequency - formant * formant_scale) / 170.0;
                        (-0.5 * distance * distance).exp()
                    })
                    .sum::<f32>();
                sample += (harmonic as f32 * phase).sin() * formant_gain / (harmonic as f32).sqrt();
            }
            noise_state = noise_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let noise = (noise_state as i32 as f32 / i32::MAX as f32) * 0.012;
            ((sample * envelope * 0.035) + noise).clamp(-0.9, 0.9)
        })
        .collect()
}

pub fn append_with_silence(target: &mut Vec<f32>, pcm: &[f32], silence_samples: usize) {
    target.extend(std::iter::repeat_n(0.0, silence_samples));
    target.extend_from_slice(pcm);
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    dot / (left_norm * right_norm).max(f32::MIN_POSITIVE)
}
