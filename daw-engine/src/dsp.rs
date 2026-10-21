// DSP module placeholder
pub struct AudioBuffer {
    pub data: Vec<f32>,
    pub channels: usize,
    pub frames: usize,
}

impl AudioBuffer {
    pub fn new(channels: usize, frames: usize) -> Self {
        Self {
            data: vec![0.0; channels * frames],
            channels,
            frames,
        }
    }

    pub fn silence(channels: usize, frames: usize) -> Self {
        Self::new(channels, frames)
    }
}

/// Peak and RMS levels of an interleaved sample block, both in `0.0..=1.0`
/// (values above full scale are preserved so meters can show clipping).
pub fn peak_and_rms(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    for &sample in samples {
        let abs = sample.abs();
        if abs > peak {
            peak = abs;
        }
        sum_squares += (sample as f64) * (sample as f64);
    }
    let rms = (sum_squares / samples.len() as f64).sqrt() as f32;
    (peak, rms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_and_rms_measure_the_block() {
        let (peak, rms) = peak_and_rms(&[0.5, -0.5, 0.5, -0.5]);
        assert!((peak - 0.5).abs() < 1e-6);
        assert!((rms - 0.5).abs() < 1e-6);

        let (peak, rms) = peak_and_rms(&[0.0, 0.0]);
        assert_eq!((peak, rms), (0.0, 0.0));

        let (peak, _) = peak_and_rms(&[1.2, -0.2]);
        assert!((peak - 1.2).abs() < 1e-6);
        assert_eq!(peak_and_rms(&[]), (0.0, 0.0));
    }
}
