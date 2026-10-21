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
    /// Track index. Automatic crossfades only apply between clips on the same
    /// track.
    pub track: u32,
    pub gain: f32,
    pub fade_in: Option<Fade>,
    pub fade_out: Option<Fade>,
}

/// Automatic equal-power crossfade applied where clips on the same track
/// overlap. Lengths are in output-device frames.
#[derive(Clone, Copy, Default)]
struct AutoFade {
    fade_in_frames: u64,
    fade_out_frames: u64,
}

/// Metronome click settings for the playback mixer.
#[derive(Debug, Clone, Copy)]
pub struct MetronomeConfig {
    /// Output frames per beat.
    pub frames_per_beat: f64,
    /// Beats per bar; the first beat of each bar is accented.
    pub beats_per_bar: u32,
    /// Click level, `0.0..=1.0`.
    pub gain: f32,
}

/// Real-time mixer: sums all clips into the output buffer and tracks the
/// playback position. Lives behind a mutex shared with the audio callback.
pub struct PlaybackMixer {
    out_sample_rate: u32,
    out_channels: u16,
    position: u64,
    clips: Vec<PlayableClip>,
    /// Audible length of each clip in output-device frames.
    lengths: Vec<u64>,
    auto_fades: Vec<AutoFade>,
    scratch: Vec<f32>,
    /// Optional repeating range in output-device frames.
    loop_range: Option<(u64, u64)>,
    metronome: Option<MetronomeConfig>,
}

impl PlaybackMixer {
    pub fn new(
        out_sample_rate: u32,
        out_channels: u16,
        clips: Vec<PlayableClip>,
        start_frame: u64,
    ) -> Self {
        let out_sample_rate = out_sample_rate.max(1);
        let out_channels = out_channels.max(1);

        let lengths: Vec<u64> = clips
            .iter()
            .map(|clip| {
                let frames = (clip.samples.len() / clip.channels.max(1) as usize) as u64;
                (frames as f64 * out_sample_rate as f64 / clip.sample_rate.max(1) as f64)
                    .ceil() as u64
            })
            .collect();

        // Find overlaps on each track and build an equal-power crossfade at
        // each boundary: the earlier clip fades out while the later one fades
        // in over the overlapping region.
        let mut auto_fades = vec![AutoFade::default(); clips.len()];
        for i in 0..clips.len() {
            let a_start = clips[i].start_frame;
            let a_end = a_start.saturating_add(lengths[i]);
            for j in (i + 1)..clips.len() {
                if clips[i].track != clips[j].track {
                    continue;
                }
                let b_start = clips[j].start_frame;
                let b_end = b_start.saturating_add(lengths[j]);
                // A crossfade needs a shared boundary. If one clip is fully
                // contained in the other (including equal starts), just layer
                // them instead of ramping amplitudes.
                let a_contains_b = a_start <= b_start && a_end >= b_end;
                let b_contains_a = b_start <= a_start && b_end >= a_end;
                if a_contains_b || b_contains_a {
                    continue;
                }
                let overlap_start = a_start.max(b_start);
                let overlap_end = a_end.min(b_end);
                if overlap_end <= overlap_start {
                    continue;
                }
                let overlap = overlap_end - overlap_start;
                if b_start > a_start {
                    auto_fades[i].fade_out_frames =
                        auto_fades[i].fade_out_frames.max(overlap);
                    auto_fades[j].fade_in_frames = auto_fades[j].fade_in_frames.max(overlap);
                } else if b_start < a_start {
                    auto_fades[j].fade_out_frames =
                        auto_fades[j].fade_out_frames.max(overlap);
                    auto_fades[i].fade_in_frames = auto_fades[i].fade_in_frames.max(overlap);
                }
                // Equal start positions have no meaningful crossfade.
            }
        }
        for (fade, &len) in auto_fades.iter_mut().zip(lengths.iter()) {
            fade.fade_in_frames = fade.fade_in_frames.min(len);
            fade.fade_out_frames = fade.fade_out_frames.min(len);
        }

        Self {
            out_sample_rate,
            out_channels,
            position: start_frame,
            clips,
            lengths,
            auto_fades,
            scratch: Vec::new(),
            loop_range: None,
            metronome: None,
        }
    }

    /// Current playback position in output frames.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Restrict playback to a repeating range of output frames. `None`
    /// disables looping.
    pub fn set_loop_range(&mut self, range: Option<(u64, u64)>) {
        self.loop_range = match range {
            Some((start, end)) if end > start => Some((start, end)),
            _ => None,
        };
        if let Some((start, end)) = self.loop_range {
            if self.position >= end {
                self.position = start + (self.position - start) % (end - start);
            }
        }
    }

    pub fn loop_range(&self) -> Option<(u64, u64)> {
        self.loop_range
    }

    /// Set (or clear) the metronome click generator.
    pub fn set_metronome(&mut self, config: Option<MetronomeConfig>) {
        self.metronome = config.filter(|config| {
            config.gain > 0.0 && config.frames_per_beat > 0.0 && config.beats_per_bar > 0
        });
    }

    pub fn metronome(&self) -> Option<MetronomeConfig> {
        self.metronome
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

        let mut scratch = std::mem::take(&mut self.scratch);
        if scratch.len() < needed {
            // Only allocates on the first callback (or if the buffer grows).
            scratch.resize(needed, 0.0);
        }
        self.mix_output(&mut scratch[..needed]);

        for (i, sample) in data[..needed].iter_mut().enumerate() {
            // Hard limit so summed clips can't clip the device.
            *sample = T::from_sample(scratch[i].clamp(-1.0, 1.0));
        }
        for sample in data[needed..].iter_mut() {
            *sample = T::from_sample(0.0f32);
        }

        self.scratch = scratch;
    }

    /// Offline render without the device safety limiter. Used when bouncing or
    /// consolidating so headroom above full scale is preserved in the file.
    pub fn fill_unclamped(&mut self, data: &mut [f32]) {
        let out_ch = self.out_channels as usize;
        let frames = data.len() / out_ch;
        let needed = frames * out_ch;

        let mut scratch = std::mem::take(&mut self.scratch);
        if scratch.len() < needed {
            scratch.resize(needed, 0.0);
        }
        self.mix_output(&mut scratch[..needed]);
        data[..needed].copy_from_slice(&scratch[..needed]);
        for sample in data[needed..].iter_mut() {
            *sample = 0.0;
        }

        self.scratch = scratch;
    }

    /// Mix `out.len() / channels` frames starting at the current position,
    /// wrapping at the loop end when looping is enabled.
    fn mix_output(&mut self, out: &mut [f32]) {
        let out_ch = self.out_channels as usize;
        let frames = out.len() / out_ch;
        let mut written = 0;

        while written < frames {
            let remaining = frames - written;
            let segment = self.frames_until_loop_end(remaining);
            let start = written * out_ch;
            let end = start + segment * out_ch;
            self.mix_segment(&mut out[start..end]);
            written += segment;
            self.advance_position(segment as u64);
        }
    }

    /// Number of frames to mix before the next loop wrap.
    fn frames_until_loop_end(&self, remaining: usize) -> usize {
        if let Some((start, end)) = self.loop_range {
            if end > start && self.position < end {
                let until_end = (end - self.position) as usize;
                return remaining.min(until_end);
            }
        }
        remaining
    }

    fn advance_position(&mut self, frames: u64) {
        self.position = self.position.saturating_add(frames);
        if let Some((start, end)) = self.loop_range {
            if end > start && self.position >= end {
                self.position = start + (self.position - start) % (end - start);
            }
        }
    }

    fn mix_segment(&mut self, out: &mut [f32]) {
        let out_ch = self.out_channels as usize;
        let frames = out.len() / out_ch;
        let metronome = self.metronome;
        out.fill(0.0);

        for frame in 0..frames {
            let pos = self.position + frame as u64;
            for (idx, clip) in self.clips.iter().enumerate() {
                let clip_frames =
                    (clip.samples.len() / clip.channels.max(1) as usize) as u64;
                if clip_frames == 0 {
                    continue;
                }

                // Clip extent on the output timeline.
                let len_out = self.lengths[idx];
                if pos < clip.start_frame || pos >= clip.start_frame + len_out {
                    continue;
                }

                let src_pos = (pos - clip.start_frame) as f64
                    * clip.sample_rate.max(1) as f64
                    / self.out_sample_rate as f64;
                let gain = clip.gain
                    * fade_gain(clip, src_pos)
                    * auto_fade_gain(
                        self.auto_fades[idx],
                        pos - clip.start_frame,
                        len_out,
                    );

                for channel in 0..out_ch {
                    out[frame * out_ch + channel] +=
                        sample_at(clip, src_pos, channel) * gain;
                }
            }

            if let Some(config) = metronome {
                let click = metronome_sample(config, pos, self.out_sample_rate);
                if click != 0.0 {
                    for channel in 0..out_ch {
                        out[frame * out_ch + channel] += click;
                    }
                }
            }
        }
    }
}

/// One metronome click sample at output frame `pos`. The first beat of each
/// bar uses a higher pitch.
fn metronome_sample(config: MetronomeConfig, pos: u64, sample_rate: u32) -> f32 {
    let frames_per_beat = config.frames_per_beat;
    if frames_per_beat <= 0.0 {
        return 0.0;
    }
    let sample_rate = sample_rate.max(1) as f64;
    let click_secs = 0.03;
    let click_frames = (click_secs * sample_rate).max(1.0);
    let phase = (pos as f64) % frames_per_beat;
    if phase >= click_frames {
        return 0.0;
    }

    let beat_index = ((pos as f64) / frames_per_beat).floor() as u64;
    let accented = beat_index % config.beats_per_bar.max(1) as u64 == 0;
    let frequency = if accented { 1_760.0 } else { 880.0 };
    let t = phase / sample_rate;
    let envelope = (-t / 0.012).exp() as f32;
    (std::f32::consts::TAU * frequency as f32 * t as f32).sin() * envelope * config.gain
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

/// Equal-power automatic crossfade gain at an output-frame offset within the
/// clip. `len` is the clip's audible length in output frames.
fn auto_fade_gain(fade: AutoFade, offset: u64, len: u64) -> f32 {
    use std::f32::consts::FRAC_PI_2;

    let mut gain = 1.0f32;

    if fade.fade_in_frames > 0 && offset < fade.fade_in_frames {
        let t = (offset as f32 / fade.fade_in_frames as f32).clamp(0.0, 1.0);
        gain *= (t * FRAC_PI_2).sin();
    }

    if fade.fade_out_frames > 0 {
        let start = len.saturating_sub(fade.fade_out_frames);
        if offset >= start {
            let t = ((offset - start) as f32 / fade.fade_out_frames as f32).clamp(0.0, 1.0);
            gain *= (t * FRAC_PI_2).cos();
        }
    }

    gain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(samples: Vec<f32>, start_frame: u64, track: u32) -> PlayableClip {
        PlayableClip {
            samples,
            channels: 1,
            sample_rate: 10,
            start_frame,
            track,
            gain: 1.0,
            fade_in: None,
            fade_out: None,
        }
    }

    #[test]
    fn unclamped_fill_preserves_headroom_above_full_scale() {
        let a = clip(vec![1.0; 4], 0, 0);
        let b = clip(vec![1.0; 4], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a, b], 0);
        let mut out = [0.0f32; 4];
        mixer.fill_unclamped(&mut out);

        assert!(out.iter().all(|sample| (sample - 2.0).abs() < 1e-6));
    }

    #[test]
    fn loop_range_wraps_position_and_output() {
        // A single constant clip long enough to cover the loop.
        let a = clip(vec![1.0; 20], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a], 0);
        mixer.set_loop_range(Some((4, 8)));

        // Start before the loop end; position should wrap when it reaches 8.
        let mut out = [0.0f32; 6];
        mixer.fill_unclamped(&mut out);
        assert!(out.iter().all(|sample| (sample - 1.0).abs() < 1e-6));
        assert_eq!(mixer.position(), 6);

        // Crossing the loop boundary continues from the loop start.
        let mut out = [0.0f32; 4];
        mixer.fill_unclamped(&mut out);
        assert!(out.iter().all(|sample| (sample - 1.0).abs() < 1e-6));
        assert_eq!(mixer.position(), 6);
    }

    #[test]
    fn setting_loop_range_resets_position_inside_the_range() {
        let a = clip(vec![1.0; 20], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a], 0);
        let mut out = [0.0f32; 12];
        mixer.fill_unclamped(&mut out);
        assert_eq!(mixer.position(), 12);

        mixer.set_loop_range(Some((4, 8)));
        assert_eq!(mixer.position(), 4);
    }

    #[test]
    fn metronome_clicks_on_beat_and_silences_between() {
        let mut mixer = PlaybackMixer::new(1000, 1, Vec::new(), 0);
        mixer.set_metronome(Some(MetronomeConfig {
            frames_per_beat: 100.0,
            beats_per_bar: 4,
            gain: 1.0,
        }));

        let mut out = [0.0f32; 200];
        mixer.fill_unclamped(&mut out);
        // The click is 30 frames long; the gap before the next beat is silent.
        assert!(out[1..30].iter().any(|sample| sample.abs() > 1e-4));
        assert!(out[40..90].iter().all(|sample| sample.abs() < 1e-6));
        // The next beat starts at frame 100.
        assert!(out[101..130].iter().any(|sample| sample.abs() > 1e-4));

        mixer.set_metronome(None);
        let mut silent = [1.0f32; 50];
        mixer.fill_unclamped(&mut silent);
        assert!(silent.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn overlapping_clips_on_same_track_crossfade() {
        // A is audible first, B is silent, so the mix shows A's fade-out.
        let a = clip(vec![1.0; 10], 0, 0);
        let b = clip(vec![0.0; 10], 5, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a, b], 0);
        let mut out = [0.0f32; 15];
        mixer.fill(&mut out);

        assert!((out[4] - 1.0).abs() < 1e-5);
        let expected = (0.4 * std::f32::consts::FRAC_PI_2).cos();
        assert!((out[7] - expected).abs() < 1e-4);
        assert!(out[10].abs() < 1e-6);
    }

    #[test]
    fn later_clip_fades_in_over_overlap() {
        // A is silent, B is audible, so the mix shows B's fade-in.
        let a = clip(vec![0.0; 10], 0, 0);
        let b = clip(vec![1.0; 10], 5, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a, b], 0);
        let mut out = [0.0f32; 15];
        mixer.fill(&mut out);

        assert!(out[4].abs() < 1e-6);
        let expected = (0.4 * std::f32::consts::FRAC_PI_2).sin();
        assert!((out[7] - expected).abs() < 1e-4);
    }

    #[test]
    fn overlaps_on_different_tracks_do_not_crossfade() {
        let a = clip(vec![1.0; 10], 0, 0);
        let b = clip(vec![0.0; 10], 5, 1);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a, b], 0);
        let mut out = [0.0f32; 15];
        mixer.fill(&mut out);

        assert!((out[7] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn contained_clips_do_not_crossfade() {
        // B lies entirely inside A: layering, not a shared-boundary crossfade.
        let a = clip(vec![1.0; 10], 0, 0);
        let b = clip(vec![0.0; 3], 2, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a, b], 0);
        let mut out = [0.0f32; 12];
        mixer.fill(&mut out);

        assert!((out[7] - 1.0).abs() < 1e-6);
    }
}
