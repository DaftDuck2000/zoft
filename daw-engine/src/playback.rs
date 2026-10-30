//! Playback mixing: feeds the output stream from project audio clips.

use cpal::{FromSample, Sample, SizedSample};
use daw_core::clip::Fade;
use daw_core::track::{FilterMode, InsertSlot, TrackEffect};
use std::sync::{Arc, Mutex};

/// Latest peak/RMS reading for one track.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackMeter {
    pub peak: f32,
    pub rms: f32,
}

/// Stereo peak/RMS reading for the master output.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StereoMeter {
    pub left_peak: f32,
    pub right_peak: f32,
    pub left_rms: f32,
    pub right_rms: f32,
}

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

/// One MIDI note prepared for playback by the simple built-in sine voice.
#[derive(Debug, Clone, Copy)]
pub struct PlayableNote {
    pub start_frame: u64,
    pub end_frame: u64,
    pub note: u8,
    /// Normalized velocity (`0.0..=1.0`).
    pub velocity: f32,
    /// Track index, used to route the note through that track's effects.
    pub track: u32,
    /// Per-note pan gains from CC10, applied before the track pan.
    pub pan_left: f32,
    pub pan_right: f32,
    /// Per-note gain from CC7 volume and CC11 expression.
    pub gain: f32,
}

/// One built-in effect in a track's insert chain, prepared for playback.
#[derive(Clone)]
pub struct PlayableEffect {
    pub effect: TrackEffect,
    pub enabled: bool,
    pub input_gain: f32,
    pub output_gain: f32,
    pub mix: f32,
}

impl PlayableEffect {
    pub fn from_slot(slot: &InsertSlot) -> Option<Self> {
        Some(Self {
            effect: slot.effect.clone()?,
            enabled: slot.enabled,
            input_gain: slot.input_gain,
            output_gain: slot.output_gain,
            mix: slot.mix,
        })
    }
}

/// Per-track playback settings: fader gain, pan and insert effects.
#[derive(Clone)]
pub struct PlayableTrack {
    /// Matches `PlayableClip::track` and `PlayableNote::track`.
    pub index: u32,
    pub gain: f32,
    /// Equal-power pan gains for the left and right output channels.
    pub pan_left: f32,
    pub pan_right: f32,
    pub effects: Vec<PlayableEffect>,
}

/// Runtime state for one effect slot. Kept across audio blocks so filters,
/// bit-crushers and tremolo stay continuous.
enum EffectState {
    None,
    BitCrusher { held: Vec<f32>, counter: u32 },
    Filter { state: Vec<f32> },
    Tremolo { phase: f32 },
}

impl EffectState {
    fn for_effect(effect: &TrackEffect, channels: usize) -> Self {
        match effect {
            TrackEffect::BitCrusher { .. } => Self::BitCrusher {
                held: vec![0.0; channels.max(1)],
                counter: 0,
            },
            TrackEffect::Filter { .. } => Self::Filter {
                state: vec![0.0; channels.max(1)],
            },
            TrackEffect::Tremolo { .. } => Self::Tremolo { phase: 0.0 },
            TrackEffect::Saturation { .. } => Self::None,
        }
    }
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
    notes: Vec<PlayableNote>,
    /// Per-track fader/effects settings. Empty means legacy unity-gain direct
    /// mixing (used by offline tests and callers that don't supply tracks).
    tracks: Vec<PlayableTrack>,
    clip_buckets: Vec<Vec<usize>>,
    note_buckets: Vec<Vec<usize>>,
    effect_states: Vec<Vec<EffectState>>,
    /// Scratch buffer for one track's mix before effects are applied.
    track_scratch: Vec<f32>,
    /// Scratch buffer holding the dry signal while an effect is blended in.
    effect_dry: Vec<f32>,
    /// Optional shared peak/RMS readings consumed by the UI meters.
    meters: Option<Arc<Mutex<Vec<TrackMeter>>>>,
    /// Optional shared master-output meter.
    master_meter: Option<Arc<Mutex<StereoMeter>>>,
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
            notes: Vec::new(),
            tracks: Vec::new(),
            clip_buckets: Vec::new(),
            note_buckets: Vec::new(),
            effect_states: Vec::new(),
            track_scratch: Vec::new(),
            effect_dry: Vec::new(),
            meters: None,
            master_meter: None,
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

    /// Replace the MIDI notes rendered by the built-in sine voice.
    pub fn set_midi_notes(&mut self, notes: Vec<PlayableNote>) {
        self.notes = notes;
        self.rebuild_note_buckets();
    }

    /// Replace the per-track fader/effect settings. Call before loading clips so
    /// every track in the arrangement keeps its own insert chain.
    pub fn set_tracks(&mut self, tracks: Vec<PlayableTrack>) {
        self.effect_states = tracks
            .iter()
            .map(|track| {
                track
                    .effects
                    .iter()
                    .map(|effect| {
                        EffectState::for_effect(&effect.effect, self.out_channels as usize)
                    })
                    .collect()
            })
            .collect();
        self.tracks = tracks;
        self.rebuild_clip_buckets();
        self.rebuild_note_buckets();
    }

    pub fn tracks(&self) -> &[PlayableTrack] {
        &self.tracks
    }

    /// Share a meter buffer that receives one peak/RMS reading per track on
    /// every mixed block.
    pub fn set_meters(&mut self, meters: Arc<Mutex<Vec<TrackMeter>>>) {
        self.meters = Some(meters);
    }

    /// Share a meter buffer for the summed master output.
    pub fn set_master_meter(&mut self, meter: Arc<Mutex<StereoMeter>>) {
        self.master_meter = Some(meter);
    }

    fn rebuild_clip_buckets(&mut self) {
        self.clip_buckets = vec![Vec::new(); self.tracks.len()];
        for (index, clip) in self.clips.iter().enumerate() {
            if let Some(bucket) = self.clip_buckets.get_mut(clip.track as usize) {
                bucket.push(index);
            }
        }
    }

    fn rebuild_note_buckets(&mut self) {
        self.note_buckets = vec![Vec::new(); self.tracks.len()];
        for (index, note) in self.notes.iter().enumerate() {
            if let Some(bucket) = self.note_buckets.get_mut(note.track as usize) {
                bucket.push(index);
            }
        }
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

        let mut track_buf = std::mem::take(&mut self.track_scratch);
        track_buf.resize(out.len(), 0.0);
        let mut dry = std::mem::take(&mut self.effect_dry);
        dry.resize(out.len(), 0.0);

        if self.tracks.is_empty() {
            track_buf.fill(0.0);
            self.render_bucket(None, &mut track_buf, frames);
            for (dst, src) in out.iter_mut().zip(track_buf.iter()) {
                *dst += *src;
            }
        } else {
            // Move the track list and effect state out so rendering a bucket
            // and updating its effect state don't overlap borrows.
            let tracks = std::mem::take(&mut self.tracks);
            let mut states = std::mem::take(&mut self.effect_states);
            let meter_sink = self.meters.clone();
            let mut meter_guard = meter_sink
                .as_ref()
                .and_then(|meters| meters.lock().ok());
            if let Some(meters) = meter_guard.as_mut() {
                if meters.len() < tracks.len() {
                    meters.resize(tracks.len(), TrackMeter::default());
                }
            }
            for (slot, track) in tracks.iter().enumerate() {
                track_buf.fill(0.0);
                self.render_bucket(Some(slot), &mut track_buf, frames);
                if let Some(effect_states) = states.get_mut(slot) {
                    process_effects(
                        &track.effects,
                        effect_states,
                        &mut track_buf,
                        &mut dry,
                        out_ch,
                        self.out_sample_rate,
                    );
                }
                let gain = track.gain;
                let pan_left = track.pan_left;
                let pan_right = track.pan_right;
                let mono_pan = (pan_left + pan_right) * 0.5;
                let mut peak = 0.0f32;
                let mut sum_sq = 0.0f32;
                let mut sample_count = 0usize;
                for frame in 0..frames {
                    let base = frame * out_ch;
                    for channel in 0..out_ch {
                        let pan = if out_ch == 1 {
                            mono_pan
                        } else if channel == 0 {
                            pan_left
                        } else if channel == 1 {
                            pan_right
                        } else {
                            1.0
                        };
                        let sample =
                            track_buf[base + channel] * gain * pan;
                        out[base + channel] += sample;
                        peak = peak.max(sample.abs());
                        sum_sq += sample * sample;
                        sample_count += 1;
                    }
                }
                if let Some(meters) = meter_guard.as_mut() {
                    if let Some(meter) = meters.get_mut(slot) {
                        *meter = TrackMeter {
                            peak,
                            rms: (sum_sq / sample_count.max(1) as f32)
                                .sqrt(),
                        };
                    }
                }
            }
            self.tracks = tracks;
            self.effect_states = states;
        }

        if let Some(config) = metronome {
            for frame in 0..frames {
                let pos = self.position + frame as u64;
                let click = metronome_sample(config, pos, self.out_sample_rate);
                if click != 0.0 {
                    for channel in 0..out_ch {
                        out[frame * out_ch + channel] += click;
                    }
                }
            }
        }

        if let Some(sink) = &self.master_meter {
            let mut peaks = [0.0f32; 2];
            let mut sums = [0.0f32; 2];
            let mut counts = [0usize; 2];
            for frame in 0..frames {
                let base = frame * out_ch;
                for channel in 0..out_ch {
                    let index = channel.min(1);
                    let sample = out[base + channel];
                    peaks[index] = peaks[index].max(sample.abs());
                    sums[index] += sample * sample;
                    counts[index] += 1;
                }
            }
            if out_ch == 1 {
                peaks[1] = peaks[0];
                sums[1] = sums[0];
                counts[1] = counts[0];
            }
            if let Ok(mut meter) = sink.lock() {
                *meter = StereoMeter {
                    left_peak: peaks[0],
                    right_peak: peaks[1],
                    left_rms: (sums[0] / counts[0].max(1) as f32).sqrt(),
                    right_rms: (sums[1] / counts[1].max(1) as f32).sqrt(),
                };
            }
        }

        self.track_scratch = track_buf;
        self.effect_dry = dry;
    }

    /// Mix one track bucket (or every clip/note when `bucket` is `None`) into
    /// `out` without advancing the mixer position.
    fn render_bucket(&self, bucket: Option<usize>, out: &mut [f32], frames: usize) {
        for frame in 0..frames {
            let pos = self.position + frame as u64;
            if let Some(slot) = bucket {
                if let Some(indices) = self.clip_buckets.get(slot) {
                    for &idx in indices {
                        self.mix_clip_frame(idx, pos, out, frame);
                    }
                }
                if let Some(indices) = self.note_buckets.get(slot) {
                    for &idx in indices {
                        self.mix_note_frame(idx, pos, out, frame);
                    }
                }
            } else {
                for idx in 0..self.clips.len() {
                    self.mix_clip_frame(idx, pos, out, frame);
                }
                for idx in 0..self.notes.len() {
                    self.mix_note_frame(idx, pos, out, frame);
                }
            }
        }
    }

    fn mix_clip_frame(&self, idx: usize, pos: u64, out: &mut [f32], frame: usize) {
        let out_ch = self.out_channels as usize;
        let clip = &self.clips[idx];
        let clip_frames = (clip.samples.len() / clip.channels.max(1) as usize) as u64;
        if clip_frames == 0 {
            return;
        }
        let len_out = self.lengths[idx];
        if pos < clip.start_frame || pos >= clip.start_frame + len_out {
            return;
        }
        let src_pos = (pos - clip.start_frame) as f64
            * clip.sample_rate.max(1) as f64
            / self.out_sample_rate as f64;
        let gain = clip.gain
            * fade_gain(clip, src_pos)
            * auto_fade_gain(self.auto_fades[idx], pos - clip.start_frame, len_out);
        for channel in 0..out_ch {
            out[frame * out_ch + channel] += sample_at(clip, src_pos, channel) * gain;
        }
    }

    fn mix_note_frame(&self, idx: usize, pos: u64, out: &mut [f32], frame: usize) {
        let out_ch = self.out_channels as usize;
        let note = self.notes[idx];
        if pos < note.start_frame || pos >= note.end_frame {
            return;
        }
        let sample =
            midi_note_sample(note, pos, self.out_sample_rate) * note.gain.clamp(0.0, 1.0);
        if sample != 0.0 {
            let pan_left = note.pan_left;
            let pan_right = note.pan_right;
            let mono_pan = (pan_left + pan_right) * 0.5;
            for channel in 0..out_ch {
                let pan = if out_ch == 1 {
                    mono_pan
                } else if channel == 0 {
                    pan_left
                } else if channel == 1 {
                    pan_right
                } else {
                    1.0
                };
                out[frame * out_ch + channel] += sample * pan;
            }
        }
    }
}

/// Apply one track's insert chain to an interleaved buffer.
fn process_effects(
    effects: &[PlayableEffect],
    states: &mut [EffectState],
    buffer: &mut [f32],
    dry: &mut [f32],
    channels: usize,
    sample_rate: u32,
) {
    let channels = channels.max(1);
    let frames = buffer.len() / channels;
    let sample_rate = sample_rate.max(1) as f32;

    for (effect, state) in effects.iter().zip(states.iter_mut()) {
        if !effect.enabled {
            continue;
        }
        let input_gain = effect.input_gain.clamp(0.0, 8.0);
        if (input_gain - 1.0).abs() > 1e-6 {
            for sample in buffer.iter_mut() {
                *sample *= input_gain;
            }
        }
        let needed = buffer.len();
        dry[..needed].copy_from_slice(buffer);

        match (&effect.effect, state) {
            (TrackEffect::Saturation { drive_db, mix }, _) => {
                let drive = 10f32.powf(drive_db.clamp(0.0, 48.0) / 20.0);
                let norm = if drive > 1e-3 {
                    drive.tanh().recip()
                } else {
                    1.0
                };
                let wet = mix.clamp(0.0, 1.0);
                if wet > 0.0 {
                    for sample in buffer.iter_mut() {
                        let clean = *sample;
                        let driven = (clean * drive).tanh() * norm;
                        *sample = clean * (1.0 - wet) + driven * wet;
                    }
                }
            }
            (TrackEffect::BitCrusher { bits, downsample }, EffectState::BitCrusher { held, counter }) => {
                let steps = 2f32.powi((*bits).clamp(1, 24) as i32 - 1);
                let factor = (*downsample).max(1);
                for frame in 0..frames {
                    let base = frame * channels;
                    if *counter == 0 {
                        for channel in 0..channels {
                            held[channel] = buffer[base + channel];
                        }
                    }
                    for channel in 0..channels {
                        buffer[base + channel] =
                            (held[channel].clamp(-1.0, 1.0) * steps).round() / steps;
                    }
                    *counter = (*counter + 1) % factor;
                }
            }
            (TrackEffect::Filter { mode, cutoff }, EffectState::Filter { state }) => {
                let max_cutoff = (sample_rate * 0.475).max(1.0);
                let cutoff = cutoff.clamp(1.0, max_cutoff);
                let alpha = 1.0 - (-std::f32::consts::TAU * cutoff / sample_rate).exp();
                for frame in 0..frames {
                    let base = frame * channels;
                    for channel in 0..channels {
                        let input = buffer[base + channel];
                        state[channel] += alpha * (input - state[channel]);
                        buffer[base + channel] = match mode {
                            FilterMode::LowPass => state[channel],
                            FilterMode::HighPass => input - state[channel],
                        };
                    }
                }
            }
            (TrackEffect::Tremolo { rate, depth }, EffectState::Tremolo { phase }) => {
                let depth = depth.clamp(0.0, 1.0);
                let rate = rate.clamp(0.0, sample_rate * 0.5);
                for frame in 0..frames {
                    let modulation = 1.0 - depth * 0.5 * (1.0 - phase.cos());
                    let base = frame * channels;
                    for channel in 0..channels {
                        buffer[base + channel] *= modulation;
                    }
                    *phase += std::f32::consts::TAU * rate / sample_rate;
                    if *phase >= std::f32::consts::TAU {
                        *phase -= std::f32::consts::TAU;
                    }
                }
            }
            _ => {}
        }

        let mix = effect.mix.clamp(0.0, 1.0);
        if mix < 1.0 {
            for (sample, clean) in buffer.iter_mut().zip(dry.iter()) {
                *sample = *clean * (1.0 - mix) + *sample * mix;
            }
        }
        let output_gain = effect.output_gain.clamp(0.0, 8.0);
        if (output_gain - 1.0).abs() > 1e-6 {
            for sample in buffer.iter_mut() {
                *sample *= output_gain;
            }
        }
    }
}

/// Small polyphonic sine synthesizer for live MIDI input audition. It runs in
/// the output callback independently of transport playback, so notes are heard
/// while recording or just monitoring an armed instrument track.
pub struct LiveMidiSynth {
    sample_rate: u32,
    voices: Vec<LiveVoice>,
    left_gain: f32,
    right_gain: f32,
}

struct LiveVoice {
    note: u8,
    velocity: f32,
    phase: f32,
    age: u64,
    release_age: u64,
    releasing: bool,
}

impl LiveMidiSynth {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            voices: Vec::new(),
            left_gain: 1.0,
            right_gain: 1.0,
        }
    }

    /// Apply the active track's fader and pan gains to live audition notes.
    pub fn set_gains(&mut self, left: f32, right: f32) {
        self.left_gain = left.clamp(0.0, 4.0);
        self.right_gain = right.clamp(0.0, 4.0);
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        let velocity = (velocity as f32 / 127.0).clamp(0.0, 1.0);
        if let Some(voice) = self.voices.iter_mut().find(|voice| voice.note == note) {
            voice.velocity = velocity;
            voice.phase = 0.0;
            voice.age = 0;
            voice.release_age = 0;
            voice.releasing = false;
            return;
        }
        self.voices.push(LiveVoice {
            note,
            velocity,
            phase: 0.0,
            age: 0,
            release_age: 0,
            releasing: false,
        });
    }

    pub fn note_off(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.note == note && !voice.releasing {
                voice.releasing = true;
                voice.release_age = 0;
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        self.voices.clear();
    }

    /// Mix active voices into an interleaved output buffer.
    pub fn fill(&mut self, out: &mut [f32], channels: u16) {
        let channels = channels.max(1) as usize;
        if out.is_empty() || self.voices.is_empty() {
            return;
        }
        let frames = out.len() / channels;
        let sample_rate = self.sample_rate.max(1) as f32;
        let attack = (0.005 * sample_rate).max(1.0);
        let release = (0.05 * sample_rate).max(1.0);
        let left_gain = self.left_gain;
        let right_gain = self.right_gain;
        let mono_gain = (left_gain + right_gain) * 0.5;

        for frame in 0..frames {
            let base = frame * channels;
            for voice in &mut self.voices {
                let attack_gain = (voice.age as f32 / attack).min(1.0);
                let release_gain = if voice.releasing {
                    (1.0 - voice.release_age as f32 / release).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let gain = attack_gain * release_gain * 0.18 * voice.velocity;
                let sample = voice.phase.sin() * gain;
                for channel in 0..channels {
                    let pan = if channels == 1 {
                        mono_gain
                    } else if channel == 0 {
                        left_gain
                    } else if channel == 1 {
                        right_gain
                    } else {
                        1.0
                    };
                    out[base + channel] += sample * pan;
                }
                let frequency =
                    440.0 * 2f32.powf((voice.note as f32 - 69.0) / 12.0);
                voice.phase += std::f32::consts::TAU * frequency / sample_rate;
                if voice.phase >= std::f32::consts::TAU {
                    voice.phase -= std::f32::consts::TAU;
                }
                voice.age = voice.age.saturating_add(1);
                if voice.releasing {
                    voice.release_age = voice.release_age.saturating_add(1);
                }
            }
        }
        self.voices.retain(|voice| {
            !voice.releasing || voice.release_age as f32 / release < 1.0
        });
    }
}

/// One sample from the simple built-in sine voice for a MIDI note.
fn midi_note_sample(note: PlayableNote, pos: u64, sample_rate: u32) -> f32 {
    let sample_rate = sample_rate.max(1) as f64;
    let start = note.start_frame.min(pos);
    let t = (pos - start) as f64 / sample_rate;
    let duration = note.end_frame.saturating_sub(note.start_frame) as f64 / sample_rate;
    let frequency = 440.0 * 2f64.powf((note.note as f64 - 69.0) / 12.0);
    let attack = 0.005;
    let release = 0.04;
    let attack_gain = (t / attack).min(1.0);
    let release_gain = if duration > 0.0 {
        ((duration - t) / release).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let envelope = attack_gain * release_gain * 0.18;
    (std::f64::consts::TAU * frequency * t).sin() as f32
        * envelope as f32
        * note.velocity.clamp(0.0, 1.0)
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
    fn midi_notes_render_only_between_their_frames() {
        let mut mixer = PlaybackMixer::new(1000, 1, Vec::new(), 0);
        mixer.set_midi_notes(vec![PlayableNote {
            start_frame: 10,
            end_frame: 30,
            note: 69,
            velocity: 1.0,
            track: 0,
            pan_left: 1.0,
            pan_right: 1.0,
            gain: 1.0,
        }]);

        let mut out = [0.0f32; 40];
        mixer.fill_unclamped(&mut out);
        assert!(out[..10].iter().all(|sample| sample.abs() < 1e-6));
        assert!(out[10..30].iter().any(|sample| sample.abs() > 1e-4));
        assert!(out[30..].iter().all(|sample| sample.abs() < 1e-6));
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

    #[test]
    fn track_fader_gain_scales_the_mixed_track() {
        let a = clip(vec![1.0; 4], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 0.25,
            pan_left: 1.0,
            pan_right: 1.0,
            effects: Vec::new(),
        }]);

        let mut out = [0.0f32; 4];
        mixer.fill_unclamped(&mut out);
        assert!(out.iter().all(|sample| (sample - 0.25).abs() < 1e-6));
    }

    #[test]
    fn track_pan_routes_the_signal_to_one_channel() {
        let a = clip(vec![1.0; 4], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 2, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 1.0,
            pan_left: 1.0,
            pan_right: 0.0,
            effects: Vec::new(),
        }]);

        let mut out = [0.0f32; 8];
        mixer.fill_unclamped(&mut out);
        assert!(out
            .iter()
            .step_by(2)
            .all(|sample| (sample - 1.0).abs() < 1e-6));
        assert!(out
            .iter()
            .skip(1)
            .step_by(2)
            .all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn track_meters_report_peak_and_rms() {
        let meters = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let a = clip(vec![1.0, -0.5, 0.25, 0.0], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 1, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 1.0,
            pan_left: 1.0,
            pan_right: 1.0,
            effects: Vec::new(),
        }]);
        mixer.set_meters(std::sync::Arc::clone(&meters));

        let mut out = [0.0f32; 4];
        mixer.fill_unclamped(&mut out);
        let meters = meters.lock().unwrap();
        assert_eq!(meters.len(), 1);
        assert!((meters[0].peak - 1.0).abs() < 1e-6);
        assert!(meters[0].rms > 0.0);
    }

    #[test]
    fn master_meter_reports_stereo_peak() {
        let meter = std::sync::Arc::new(std::sync::Mutex::new(
            StereoMeter::default(),
        ));
        let a = clip(vec![1.0; 4], 0, 0);
        let mut mixer = PlaybackMixer::new(10, 2, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 1.0,
            pan_left: 1.0,
            pan_right: 0.0,
            effects: Vec::new(),
        }]);
        mixer.set_master_meter(std::sync::Arc::clone(&meter));

        let mut out = [0.0f32; 8];
        mixer.fill_unclamped(&mut out);
        let meter = meter.lock().unwrap();
        assert!((meter.left_peak - 1.0).abs() < 1e-6);
        assert!(meter.right_peak.abs() < 1e-6);
    }

    #[test]
    fn track_effects_process_audio_before_the_fader() {
        let a = clip(vec![1.0; 100], 0, 0);
        let mut mixer = PlaybackMixer::new(1000, 1, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 1.0,
            pan_left: 1.0,
            pan_right: 1.0,
            effects: vec![PlayableEffect {
                effect: TrackEffect::Tremolo {
                    rate: 10.0,
                    depth: 1.0,
                },
                enabled: true,
                input_gain: 1.0,
                output_gain: 1.0,
                mix: 1.0,
            }],
        }]);

        let mut out = [0.0f32; 100];
        mixer.fill_unclamped(&mut out);
        assert!((out[0] - 1.0).abs() < 1e-5);
        assert!(out[50].abs() < 0.05);
    }

    #[test]
    fn disabled_track_effects_are_bypassed() {
        let a = clip(vec![1.0; 32], 0, 0);
        let mut mixer = PlaybackMixer::new(1000, 1, vec![a], 0);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 1.0,
            pan_left: 1.0,
            pan_right: 1.0,
            effects: vec![PlayableEffect {
                effect: TrackEffect::Tremolo {
                    rate: 10.0,
                    depth: 1.0,
                },
                enabled: false,
                input_gain: 1.0,
                output_gain: 1.0,
                mix: 1.0,
            }],
        }]);

        let mut out = [0.0f32; 32];
        mixer.fill_unclamped(&mut out);
        assert!(out.iter().all(|sample| (sample - 1.0).abs() < 1e-6));
    }

    #[test]
    fn midi_note_pan_routes_per_note() {
        let mut mixer = PlaybackMixer::new(1000, 2, Vec::new(), 0);
        mixer.set_midi_notes(vec![PlayableNote {
            start_frame: 0,
            end_frame: 100,
            note: 69,
            velocity: 1.0,
            track: 0,
            pan_left: 1.0,
            pan_right: 0.0,
            gain: 1.0,
        }]);

        let mut out = [0.0f32; 200];
        mixer.fill_unclamped(&mut out);
        assert!(out
            .iter()
            .step_by(2)
            .any(|sample| sample.abs() > 1e-4));
        assert!(out
            .iter()
            .skip(1)
            .step_by(2)
            .all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn midi_note_gain_scales_the_voice() {
        let note = |gain: f32| PlayableNote {
            start_frame: 0,
            end_frame: 100,
            note: 69,
            velocity: 1.0,
            track: 0,
            pan_left: 1.0,
            pan_right: 1.0,
            gain,
        };
        let mut full_mixer = PlaybackMixer::new(1000, 1, Vec::new(), 0);
        full_mixer.set_midi_notes(vec![note(1.0)]);
        let mut quiet_mixer = PlaybackMixer::new(1000, 1, Vec::new(), 0);
        quiet_mixer.set_midi_notes(vec![note(0.5)]);

        let mut full = [0.0f32; 100];
        let mut quiet = [0.0f32; 100];
        full_mixer.fill_unclamped(&mut full);
        quiet_mixer.fill_unclamped(&mut quiet);
        for (loud, soft) in full.iter().zip(quiet.iter()) {
            assert!((loud * 0.5 - soft).abs() < 1e-4);
        }
    }

    #[test]
    fn midi_notes_follow_their_track_fader() {
        let mut mixer = PlaybackMixer::new(1000, 1, Vec::new(), 0);
        mixer.set_midi_notes(vec![PlayableNote {
            start_frame: 0,
            end_frame: 100,
            note: 69,
            velocity: 1.0,
            track: 0,
            pan_left: 1.0,
            pan_right: 1.0,
            gain: 1.0,
        }]);
        mixer.set_tracks(vec![PlayableTrack {
            index: 0,
            gain: 0.0,
            pan_left: 1.0,
            pan_right: 1.0,
            effects: Vec::new(),
        }]);

        let mut out = [0.0f32; 100];
        mixer.fill_unclamped(&mut out);
        assert!(out.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn live_midi_synth_auditions_notes_without_transport() {
        let mut synth = LiveMidiSynth::new(1000);
        synth.note_on(69, 127);
        let mut out = [0.0f32; 100];
        synth.fill(&mut out, 1);
        assert!(out.iter().any(|sample| sample.abs() > 1e-4));

        synth.note_off(69);
        let mut release = [0.0f32; 200];
        synth.fill(&mut release, 1);
        assert!(release[100..].iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn live_midi_synth_applies_pan_gains() {
        let mut synth = LiveMidiSynth::new(1000);
        synth.set_gains(1.0, 0.0);
        synth.note_on(69, 127);
        let mut out = [0.0f32; 200];
        synth.fill(&mut out, 2);
        assert!(out
            .iter()
            .step_by(2)
            .any(|sample| sample.abs() > 1e-4));
        assert!(out
            .iter()
            .skip(1)
            .step_by(2)
            .all(|sample| sample.abs() < 1e-6));
    }
}
