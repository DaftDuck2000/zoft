//! Playback mixing: feeds the output stream from project audio clips.

use cpal::{FromSample, Sample, SizedSample};
use daw_core::clip::Fade;

/// A clip prepared for playback: interleaved f32 samples plus timeline
/// placement and gain/fade settings.
#[derive(Clone)]
pub struct PlayableClip {
    /// Interleaved samples (`frames * channels`).
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
    /// Timeline position in output-device frames.
    pub start_frame: u64,
    pub gain: f32,
    pub fade_in: Option<Fade>,
    pub fade_out: Option<Fade>,
}

/// Real-time mixer: sums all clips into the output buffer and tracks the
/// playback position. Lives behind a mutex shared with the audio callback.
pub struct PlaybackMixer {
    out_sample_rate: u32,
    out_channels: u16,
    position: u64,
    clips: Vec<PlayableClip>,
    scratch: Vec<f32>,
}

impl PlaybackMixer {
    pub fn new(
        out_sample_rate: u32,
        out_channels: u16,
        clips: Vec<PlayableClip>,
        start_frame: u64,
    ) -> Self {
        Self {
            out_sample_rate: out_sample_rate.max(1),
            out_channels: out_channels.max(1),
            position: start_frame,
            clips,
            scratch: Vec::new(),
        }
    }

    /// Current playback position in output frames.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Fill `data` with the mixed output, advancing the playback position.
    /// Called from the audio callback.
    pub fn fill<T>(&mut self, data: &mut [T])
    where
        T: Sample + SizedSample + FromSample<f32> + Send + 'static,
    {
        let out_ch = self.out_channels as usize;
        let frames = data.len() / out_ch;
        let needed = frames * out_ch;

        if self.scratch.len() < needed {
            // Only allocates on the first callback (or if the buffer grows).
            self.scratch.resize(needed, 0.0);
        }

        {
            let scratch = &mut self.scratch[..needed];
            scratch.fill(0.0);

            for frame in 0..frames {
                let pos = self.position + frame as u64;
                for clip in &self.clips {
                    let clip_frames =
                        (clip.samples.len() / clip.channels.max(1) as usize) as u64;
                    if clip_frames == 0 {
                        continue;
                    }

                    // Clip extent on the output timeline.
                    let len_out = (clip_frames as f64 * self.out_sample_rate as f64
                        / clip.sample_rate.max(1) as f64)
                        .ceil() as u64;
                    if pos < clip.start_frame || pos >= clip.start_frame + len_out {
                        continue;
                    }

                    let src_pos = (pos - clip.start_frame) as f64
                        * clip.sample_rate.max(1) as f64
                        / self.out_sample_rate as f64;
                    let gain = clip.gain * fade_gain(clip, src_pos);

                    for channel in 0..out_ch {
                        scratch[frame * out_ch + channel] +=
                            sample_at(clip, src_pos, channel) * gain;
                    }
                }
            }

            // Hard limit so summed clips can't clip the device.
            for value in scratch.iter_mut() {
                *value = value.clamp(-1.0, 1.0);
            }
        }

        for (i, sample) in data[..needed].iter_mut().enumerate() {
            *sample = T::from_sample(self.scratch[i]);
        }
        for sample in data[needed..].iter_mut() {
            *sample = T::from_sample(0.0f32);
        }

        self.position += frames as u64;
    }
}

/// Linearly interpolated sample lookup for `channel` at `src_pos` (frames).
fn sample_at(clip: &PlayableClip, src_pos: f64, channel: usize) -> f32 {
    let channels = clip.channels.max(1) as usize;
    let frames = clip.samples.len() / channels;
    if frames == 0 {
        return 0.0;
    }

    let i0 = src_pos.floor() as usize;
    if i0 >= frames {
        return 0.0;
    }
    let i1 = (i0 + 1).min(frames - 1);
    let frac = (src_pos - i0 as f64) as f32;
    let source_channel = channel.min(channels - 1);

    let a = clip.samples[i0 * channels + source_channel];
    let b = clip.samples[i1 * channels + source_channel];
    a + (b - a) * frac
}

/// Combined fade-in/fade-out gain at a source-frame offset within the clip.
fn fade_gain(clip: &PlayableClip, offset: f64) -> f32 {
    let offset = offset.max(0.0) as u64;
    let mut gain = 1.0f32;

    if let Some(fade) = &clip.fade_in {
        if offset < fade.length {
            gain *= fade.gain_at(offset);
        }
    }

    if let Some(fade) = &clip.fade_out {
        if fade.length > 0 {
            let clip_frames = (clip.samples.len() / clip.channels.max(1) as usize) as u64;
            let start = clip_frames.saturating_sub(fade.length);
            if offset >= start {
                let t = ((offset - start) as f32 / fade.length as f32).clamp(0.0, 1.0);
                gain *= 1.0 - fade.curve.apply(t);
            }
        }
    }

    gain.clamp(0.0, 4.0)
}
