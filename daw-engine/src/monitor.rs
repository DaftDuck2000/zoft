//! Input-monitoring ring buffer shared between the input and output callbacks.

/// Lock-protected ring buffer of interleaved input frames. The input callback
/// pushes recent audio; the output callback reads one frame per output frame
/// and mixes it into the device output while monitoring is enabled.
pub struct MonitorBuffer {
    samples: Vec<f32>,
    read_frame: usize,
    write_frame: usize,
    capacity_frames: usize,
    channels: usize,
    sample_rate: u32,
    enabled: bool,
}

impl MonitorBuffer {
    pub fn new() -> Self {
        let mut buffer = Self {
            samples: Vec::new(),
            read_frame: 0,
            write_frame: 0,
            capacity_frames: 0,
            channels: 2,
            sample_rate: 48_000,
            enabled: false,
        };
        buffer.configure(48_000, 2);
        buffer
    }

    /// Match the ring to the input stream format and allocate two seconds of
    /// headroom.
    pub fn configure(&mut self, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate.max(1);
        self.channels = channels.max(1) as usize;
        self.capacity_frames = (self.sample_rate as f32 * 2.0) as usize;
        self.capacity_frames = self.capacity_frames.max(1);
        self.samples = vec![0.0; self.capacity_frames * self.channels];
        self.read_frame = 0;
        self.write_frame = 0;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn clear(&mut self) {
        self.read_frame = 0;
        self.write_frame = 0;
    }

    fn available_frames(&self) -> usize {
        if self.write_frame >= self.read_frame {
            self.write_frame - self.read_frame
        } else {
            self.capacity_frames - self.read_frame + self.write_frame
        }
    }

    /// Push interleaved input frames. If the ring is full, the oldest frame is
    /// dropped so monitoring follows the live input instead of accumulating
    /// latency.
    pub fn push(&mut self, data: &[f32]) {
        if !self.enabled || self.channels == 0 || data.is_empty() {
            return;
        }
        let frames = data.len() / self.channels;
        for frame in 0..frames {
            let write = self.write_frame;
            for channel in 0..self.channels {
                self.samples[write * self.channels + channel] =
                    data[frame * self.channels + channel];
            }
            self.write_frame = (write + 1) % self.capacity_frames;
            if self.write_frame == self.read_frame {
                // Overrun: drop the oldest frame.
                self.read_frame = (self.read_frame + 1) % self.capacity_frames;
            }
        }
    }

    /// Pop one frame into `out` (up to two output channels). Mono input is
    /// duplicated to stereo; extra input channels are ignored. Returns false
    /// when no input sample is available yet.
    pub fn read_frame(&mut self, out: &mut [f32; 2]) -> bool {
        if !self.enabled || self.available_frames() == 0 {
            return false;
        }
        let channels = self.channels.max(1);
        for (channel, sample) in out.iter_mut().enumerate() {
            let source = channel.min(channels - 1);
            *sample = self.samples[self.read_frame * channels + source];
        }
        self.read_frame = (self.read_frame + 1) % self.capacity_frames;
        true
    }
}

impl Default for MonitorBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_input_is_duplicated_to_stereo() {
        let mut monitor = MonitorBuffer::new();
        monitor.configure(48_000, 1);
        monitor.set_enabled(true);
        monitor.push(&[0.25, 0.5]);

        let mut frame = [0.0; 2];
        assert!(monitor.read_frame(&mut frame));
        assert_eq!(frame, [0.25, 0.25]);
        assert!(monitor.read_frame(&mut frame));
        assert_eq!(frame, [0.5, 0.5]);
        assert!(!monitor.read_frame(&mut frame));
    }

    #[test]
    fn stereo_input_keeps_channels() {
        let mut monitor = MonitorBuffer::new();
        monitor.configure(48_000, 2);
        monitor.set_enabled(true);
        monitor.push(&[0.1, -0.1, 0.2, -0.2]);

        let mut frame = [0.0; 2];
        assert!(monitor.read_frame(&mut frame));
        assert_eq!(frame, [0.1, -0.1]);
        assert!(monitor.read_frame(&mut frame));
        assert_eq!(frame, [0.2, -0.2]);
    }

    #[test]
    fn disabling_clears_and_stops_reading() {
        let mut monitor = MonitorBuffer::new();
        monitor.configure(48_000, 1);
        monitor.set_enabled(true);
        monitor.push(&[1.0]);
        monitor.set_enabled(false);

        monitor.push(&[1.0]);
        let mut frame = [0.0; 2];
        assert!(!monitor.read_frame(&mut frame));
    }

    #[test]
    fn overrun_drops_oldest_frames() {
        let mut monitor = MonitorBuffer::new();
        monitor.configure(1_000, 1);
        monitor.set_enabled(true);
        // Capacity is 2 seconds = 2000 frames; the ring keeps 1999 so full and
        // empty are distinguishable.
        let data: Vec<f32> = (0..2005).map(|value| value as f32).collect();
        monitor.push(&data);
        assert_eq!(monitor.available_frames(), 1999);

        let mut frame = [0.0; 2];
        assert!(monitor.read_frame(&mut frame));
        assert_eq!(frame[0], 6.0);
    }
}
