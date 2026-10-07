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