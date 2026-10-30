//! Main application

use anyhow::Result;
use daw_core::audio_pool::AudioFileRef;
use daw_core::clip::{AudioClip, ClipId, Fade, FadeCurve, MidiClip, MidiEvent, MidiMessage, TimeStretchMode, TimelinePos};
use daw_core::project::{BitDepth, Color, MarkerId, MarkerRegion, TempoMap};
use daw_core::serialization::ProjectSerialization;
use daw_core::track::{FilterMode, InsertSlot, MonitorMode, PanLaw, Track, TrackId, TrackEffect, TrackType};
use daw_core::undo::History;
use daw_core::Project;
use daw_core::WaveformOverview;
use daw_engine::playback::{MetronomeConfig, PlayableClip, PlayableEffect, PlayableNote, PlayableTrack, PlaybackMixer};
use daw_engine::AudioEngine;
use eframe::egui;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Minimum horizontal zoom (pixels per second).
const MIN_PPS: f32 = 0.02;
/// Maximum horizontal zoom (~8 samples per pixel at 48 kHz).
const MAX_PPS: f32 = 400_000.0;
/// Samples per overview entry when generating waveforms.
const FRAMES_PER_PIXEL: u32 = 256;
/// Smallest timeline span when fitting the view to the project.
const MIN_TIMELINE_SECS: f64 = 4.0;
/// Number of decoded detail windows kept in memory.
const DETAIL_CACHE_ENTRIES: usize = 8;
/// Largest detail window we are willing to decode in one go.
const DETAIL_MAX_FRAMES: usize = 262_144;
/// Switch to sample-accurate rendering below this many samples per pixel.
const DETAIL_TRIGGER_SAMPLES_PER_PX: f64 = 4.0;
/// Horizontal click tolerance for the trim handles on a clip's edges.
const CLIP_EDGE_PX: f32 = 8.0;
/// Size of the fade-handle hit area in a clip's top corners.
const FADE_HANDLE_PX: f32 = 16.0;
/// Smallest duration a clip may be trimmed/split to (seconds).
const MIN_CLIP_SECS: f64 = 0.02;
/// Smallest loop range created by dragging the ruler (seconds).
const MIN_LOOP_SECS: f64 = 0.05;
/// Height of the timeline ruler, shared by the arrange view and track list.
const RULER_HEIGHT: f32 = 30.0;
/// Height of one track lane, shared by the arrange view and track list.
const TRACK_HEIGHT: f32 = 80.0;

pub struct App {
    project: Project,
    engine: AudioEngine,
}

impl App {
    pub fn new(project: Project, engine: AudioEngine) -> Result<Self> {
        Ok(Self { project, engine })
    }

    pub fn run(self) -> Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_title("Zoft DAW"),
            ..Default::default()
        };

        let result = eframe::run_native(
            "Zoft DAW",
            options,
            Box::new(|cc| {
                cc.egui_ctx.set_visuals(egui::Visuals::dark());
                Ok(Box::new(AppUI::new(self.project, self.engine)))
            }),
        );

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("eframe error: {:?}", e)),
        }
    }
}

/// Horizontal timeline mapping: `start_secs` is the time at the left edge,
/// `pps` is the zoom in pixels per second.
#[derive(Debug, Clone, Copy)]
struct TimelineView {
    start_secs: f64,
    pps: f32,
}

impl TimelineView {
    fn x(&self, rect: egui::Rect, t_secs: f64) -> f32 {
        rect.left() + ((t_secs - self.start_secs) * self.pps as f64) as f32
    }

    fn t(&self, rect: egui::Rect, x: f32) -> f64 {
        self.start_secs + ((x - rect.left()) / self.pps.max(f32::EPSILON)) as f64
    }

    fn visible_secs(&self, rect: egui::Rect) -> f64 {
        rect.width() as f64 / self.pps.max(f32::EPSILON) as f64
    }
}

/// Snap resolution used while dragging clip positions and trims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapMode {
    Off,
    Bar,
    Beat,
    Eighth,
    Sixteenth,
}

impl SnapMode {
    fn label(self) -> &'static str {
        match self {
            SnapMode::Off => "Off",
            SnapMode::Bar => "Bar",
            SnapMode::Beat => "Beat",
            SnapMode::Eighth => "1/8",
            SnapMode::Sixteenth => "1/16",
        }
    }

    /// Grid spacing in seconds. `beat_secs` is one signature beat (the
    /// denominator note), not necessarily a quarter note.
    fn interval_secs(self, beat_secs: f64, beats_per_bar: f64) -> Option<f64> {
        match self {
            SnapMode::Off => None,
            SnapMode::Bar => Some(beat_secs * beats_per_bar.max(1.0)),
            SnapMode::Beat => Some(beat_secs),
            SnapMode::Eighth => Some(beat_secs * 0.5),
            SnapMode::Sixteenth => Some(beat_secs * 0.25),
        }
    }
}

/// How the timeline ruler labels positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RulerMode {
    BarsBeats,
    Time,
}

impl RulerMode {
    fn label(self) -> &'static str {
        match self {
            RulerMode::BarsBeats => "Bars/Beats",
            RulerMode::Time => "Time",
        }
    }
}

/// One tick line in the ruler. Major ticks carry a label and are also drawn
/// as grid lines over the track lanes.
struct RulerTick {
    secs: f64,
    label: Option<String>,
    major: bool,
}

/// Signatures offered in the transport bar picker.
const TIME_SIGNATURES: [(u8, u8); 10] = [
    (4, 4),
    (3, 4),
    (2, 4),
    (5, 4),
    (7, 4),
    (6, 8),
    (7, 8),
    (9, 8),
    (12, 8),
    (3, 8),
];

/// The editable (non-destructive) part of an audio clip. Kept small so undo
/// entries never clone the whole source reference with its waveform overview.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipShape {
    source_offset: u64,
    position: TimelinePos,
    length: u64,
    gain: f32,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
    color: Option<Color>,
    muted: bool,
}

impl ClipShape {
    fn of(clip: &AudioClip) -> Self {
        Self {
            source_offset: clip.source_offset,
            position: clip.timeline_position,
            length: clip.length,
            gain: clip.gain,
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
            color: clip.color,
            muted: clip.muted,
        }
    }

    fn apply(self, clip: &mut AudioClip) {
        clip.source_offset = self.source_offset;
        clip.timeline_position = self.position;
        clip.length = self.length;
        clip.gain = self.gain;
        clip.fade_in = self.fade_in;
        clip.fade_out = self.fade_out;
        clip.color = self.color;
        clip.muted = self.muted;
    }
}

/// Undoable track properties toggled from the track list.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TrackShape {
    mute: bool,
    solo: bool,
    record_arm: bool,
    volume: f32,
    pan: PanLaw,
    monitor_mode: MonitorMode,
}

impl TrackShape {
    fn of(track: &daw_core::track::Track) -> Self {
        Self {
            mute: track.mute,
            solo: track.solo,
            record_arm: track.record_arm,
            volume: track.volume,
            pan: track.pan,
            monitor_mode: track.monitor_mode,
        }
    }

    fn apply(self, track: &mut daw_core::track::Track) {
        track.mute = self.mute;
        track.solo = self.solo;
        track.record_arm = self.record_arm;
        track.set_volume(self.volume);
        track.pan = self.pan;
        track.monitor_mode = self.monitor_mode;
    }
}

/// Mixer gain for a track, or `None` when it should be silent because it is
/// muted or another track is soloed.
fn audible_track_gain(track: &daw_core::track::Track, any_solo: bool) -> Option<f32> {
    if track.mute || (any_solo && !track.solo) {
        None
    } else {
        Some(track.volume.clamp(0.0, 4.0))
    }
}

/// Build a playable note from a tick range, scaling velocity by track gain.
fn playable_note(
    start_tick: u64,
    end_tick: u64,
    note: u8,
    velocity: u8,
    track: u32,
    pan_left: f32,
    pan_right: f32,
    gain: f32,
    to_frame: &impl Fn(u64) -> u64,
) -> PlayableNote {
    let start_frame = to_frame(start_tick);
    let end_frame = to_frame(end_tick).max(start_frame + 1);
    PlayableNote {
        start_frame,
        end_frame,
        note,
        velocity: (velocity as f32 / 127.0).clamp(0.0, 1.0),
        track,
        pan_left,
        pan_right,
        gain,
    }
}

/// Latest value at or before `tick`, or `default` when no point exists.
fn midi_cc_value_at(points: &[MidiCcPoint], tick: u64, default: u8) -> u8 {
    points
        .iter()
        .rev()
        .find(|point| point.tick <= tick)
        .map(|point| point.value)
        .unwrap_or(default)
}

/// Convert a MIDI CC10 pan value (0–127) to equal-power left/right gains.
fn cc_pan_gains(value: u8) -> (f32, f32) {
    let pan = (value as f32 / 127.0) * 2.0 - 1.0;
    let law = PanLaw::from_linear(pan);
    (law.left_gain, law.right_gain)
}

/// Whether a MIDI pitch is a black key.
fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// Map MIDI velocity to a high-contrast blue → cyan → yellow gradient.
fn velocity_color(velocity: u8) -> egui::Color32 {
    fn lerp(a: u8, b: u8, t: f32) -> u8 {
        (a as f32 + (b as f32 - a as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    }
    let t = velocity.min(127) as f32 / 127.0;
    let (r, g, b) = if t < 0.5 {
        let u = t * 2.0;
        (lerp(40, 45, u), lerp(70, 210, u), lerp(200, 215, u))
    } else {
        let u = (t - 0.5) * 2.0;
        (lerp(45, 255, u), lerp(210, 220, u), lerp(215, 70, u))
    };
    egui::Color32::from_rgb(r, g, b)
}

/// Indices of note rectangles intersecting a marquee.
fn notes_in_marquee(
    note_rects: &[(usize, egui::Rect)],
    marquee: egui::Rect,
) -> BTreeSet<usize> {
    note_rects
        .iter()
        .filter(|(_, rect)| rect.intersects(marquee))
        .map(|(index, _)| *index)
        .collect()
}

const CC_CONTROLLERS: [(u8, &str); 8] = [
    (1, "Mod Wheel"),
    (2, "Breath"),
    (7, "Volume"),
    (10, "Pan"),
    (11, "Expression"),
    (64, "Sustain"),
    (71, "Resonance"),
    (74, "Cutoff"),
];

fn cc_controller_label(controller: u8) -> &'static str {
    CC_CONTROLLERS
        .iter()
        .find(|(id, _)| *id == controller)
        .map(|(_, label)| *label)
        .unwrap_or("Controller")
}

/// Scientific pitch name for a MIDI note number (`C4` = 60).
fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[(pitch % 12) as usize], pitch as i32 / 12 - 1)
}

const PITCH_CLASS_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

fn pitch_in_scale(pitch: u8, root: u8, intervals: &[u8]) -> bool {
    let relative = (pitch as i16 - root as i16).rem_euclid(12) as u8;
    intervals.contains(&relative)
}

/// Move each note to the nearest pitch in the selected scale, preferring the
/// lower note when two candidates are equally close.
fn scale_conform_midi_notes(notes: &mut [PianoRollNote], root: u8, intervals: &[u8]) {
    if intervals.is_empty() {
        return;
    }
    for note in notes {
        let original = note.pitch;
        for delta in 0..=6 {
            let down = original as i16 - delta;
            if down >= 0 && pitch_in_scale(down as u8, root, intervals) {
                note.pitch = down as u8;
                break;
            }
            let up = original as i16 + delta;
            if up <= 127 && pitch_in_scale(up as u8, root, intervals) {
                note.pitch = up as u8;
                break;
            }
        }
    }
}

/// Keep only MIDI events from the selected channel, if a filter is set.
fn filter_midi_events(
    events: Vec<daw_engine::MidiInputEvent>,
    channel: Option<u8>,
) -> Vec<daw_engine::MidiInputEvent> {
    match channel {
        Some(channel) => events
            .into_iter()
            .filter(|event| event.channel == channel)
            .collect(),
        None => events,
    }
}

/// One editable note in the piano roll. Unlike the lightweight span tuples used
/// by the arrange preview, this keeps the MIDI channel and can be written back
/// into a clip.
#[derive(Debug, Clone, PartialEq)]
struct PianoRollNote {
    start: u64,
    end: u64,
    pitch: u8,
    velocity: u8,
    channel: u8,
}

/// One editable control-change point in the piano roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MidiCcPoint {
    tick: u64,
    value: u8,
    channel: u8,
}

/// Extract control-change points for one controller in content coordinates.
fn midi_clip_cc_points(clip: &MidiClip, controller: u8) -> Vec<MidiCcPoint> {
    let mut points: Vec<MidiCcPoint> = clip
        .events
        .iter()
        .filter_map(|event| match event.message {
            MidiMessage::ControlChange {
                channel,
                controller: event_controller,
                value,
            } if event_controller == controller => Some(MidiCcPoint {
                tick: event.tick,
                value,
                channel,
            }),
            _ => None,
        })
        .collect();
    points.sort_by_key(|point| (point.tick, point.channel));
    points
}

/// Replace the control-change points for one controller, preserving notes and
/// other controllers.
fn write_midi_cc_points(clip: &mut MidiClip, controller: u8, points: &[MidiCcPoint]) {
    clip.events.retain(|event| {
        !matches!(
            event.message,
            MidiMessage::ControlChange {
                controller: event_controller,
                ..
            } if event_controller == controller
        )
    });
    for point in points {
        clip.events.push(MidiEvent::control_change(
            point.tick,
            point.channel,
            controller,
            point.value,
        ));
    }
    clip.events.sort_by_key(|event| event.tick);
}

/// Pair note-on/off events in a MIDI clip into editable notes in **content
/// coordinates** (0 = start of the recorded content). Overlapping notes on the
/// same pitch/channel are supported by matching each note-off to the oldest
/// open note-on. Notes still held at the end of the content stop at its
/// boundary.
fn midi_clip_content_notes(clip: &MidiClip) -> Vec<PianoRollNote> {
    let mut notes = Vec::new();
    let mut active: HashMap<(u8, u8), VecDeque<(u64, u8)>> = HashMap::new();
    let end_of_clip = clip
        .content_offset
        .saturating_add(clip.length.max(1));

    for event in &clip.events {
        let tick = event.tick;
        match &event.message {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } if *velocity > 0 => {
                active
                    .entry((*channel, *note))
                    .or_default()
                    .push_back((tick, *velocity));
            }
            MidiMessage::NoteOn { channel, note, .. }
            | MidiMessage::NoteOff { channel, note, .. } => {
                if let Some(queue) = active.get_mut(&(*channel, *note)) {
                    if let Some((start_tick, start_velocity)) = queue.pop_front() {
                        notes.push(PianoRollNote {
                            start: start_tick,
                            end: tick.max(start_tick + 1),
                            pitch: *note,
                            velocity: start_velocity,
                            channel: *channel,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    for ((channel, note), queue) in active {
        for (start_tick, velocity) in queue {
            notes.push(PianoRollNote {
                start: start_tick,
                end: end_of_clip.max(start_tick + 1),
                pitch: note,
                velocity,
                channel,
            });
        }
    }
    notes.sort_by_key(|note| (note.start, note.pitch, note.channel));
    notes
}

/// Like [`midi_clip_content_notes`], but clipped to the clip's visible region
/// and returned in clip-relative coordinates (0 = timeline position). Used by
/// playback and the arrange preview.
fn midi_clip_notes(clip: &MidiClip) -> Vec<PianoRollNote> {
    let visible_start = clip.content_offset;
    let visible_end = visible_start.saturating_add(clip.length.max(1));
    midi_clip_content_notes(clip)
        .into_iter()
        .filter_map(|note| {
            if note.end <= visible_start || note.start >= visible_end {
                return None;
            }
            Some(PianoRollNote {
                start: note.start.max(visible_start) - visible_start,
                end: note.end.min(visible_end) - visible_start,
                pitch: note.pitch,
                velocity: note.velocity,
                channel: note.channel,
            })
        })
        .collect()
}

/// Pair note-on/off events in a MIDI clip into `(start_tick, end_tick, note,
/// velocity)` spans relative to the clip start.
fn midi_clip_note_spans(clip: &MidiClip) -> Vec<(u64, u64, u8, u8)> {
    midi_clip_notes(clip)
        .into_iter()
        .map(|note| (note.start, note.end, note.pitch, note.velocity))
        .collect()
}

/// Replace a clip's note events with the piano roll's note list. Notes are in
/// content coordinates, so trimmed material outside the visible region is kept
/// and can be revealed again by dragging the clip edge back out. Non-note
/// events (CC, pitch bend, etc.) are preserved.
fn write_piano_roll_notes(clip: &mut MidiClip, notes: &[PianoRollNote]) {
    let mut events: Vec<MidiEvent> = clip
        .events
        .iter()
        .filter(|event| {
            !matches!(
                event.message,
                MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. }
            )
        })
        .cloned()
        .collect();

    for note in notes {
        let start = note.start;
        let end = note.end.max(start + 1);
        events.push(MidiEvent::note_on(
            start,
            note.channel,
            note.pitch,
            note.velocity.max(1),
        ));
        events.push(MidiEvent::note_off(end, note.channel, note.pitch, 0));
    }

    // Note-offs sort before note-ons at the same tick so a repeated pitch
    // starts a fresh note instead of immediately closing the new one.
    events.sort_by_key(|event| {
        let priority = match event.message {
            MidiMessage::NoteOff { .. } => 0,
            MidiMessage::NoteOn { .. } => 1,
            _ => 2,
        };
        (event.tick, priority)
    });
    clip.events = events;
}

/// Length used when double-clicking a new piano-roll note: the current snap
/// value, falling back to the last-used length when snapping is off.
fn piano_roll_new_note_length(
    snap: PianoRollSnap,
    snap_ticks: u64,
    last_length: u64,
) -> u64 {
    if snap == PianoRollSnap::Off {
        last_length.max(1)
    } else {
        snap_ticks.max(1)
    }
}

/// Move note starts toward a grid, preserving each note's duration. Swing
/// delays every second grid position, with 100% placing it two thirds of the
/// way through the pair.
fn quantize_midi_notes(
    notes: &mut [PianoRollNote],
    grid_ticks: u64,
    strength: f32,
    swing: f32,
) {
    if grid_ticks <= 1 {
        return;
    }
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return;
    }
    let swing = swing.clamp(0.0, 1.0);
    for note in notes {
        let grid_index = (note.start + grid_ticks / 2) / grid_ticks;
        let mut target = grid_index * grid_ticks;
        if grid_index % 2 == 1 {
            target = target.saturating_add(
                (swing * grid_ticks as f32 / 3.0).round() as u64,
            );
        }
        let duration = note.end.saturating_sub(note.start).max(1);
        let start = (note.start as f64
            + (target as f64 - note.start as f64) * strength as f64)
            .round() as u64;
        note.start = start;
        note.end = start.saturating_add(duration);
    }
}

/// Fold a recorded MIDI take into a loop range. Notes are re-timed relative to
/// the loop start and truncated at the loop boundary so repeated passes
/// overdub on top of each other.
fn fold_midi_clip_to_loop(
    clip: &MidiClip,
    loop_start_tick: u64,
    loop_len_ticks: u64,
    loop_start_sample: u64,
) -> MidiClip {
    let mut folded = MidiClip::new(
        TimelinePos::new(loop_start_tick, loop_start_sample),
        loop_len_ticks.max(1),
    );
    if loop_len_ticks == 0 {
        return folded;
    }
    let take_start = clip.timeline_position.tick;
    for note in midi_clip_content_notes(clip) {
        let absolute_start = take_start.saturating_add(note.start);
        let relative_start = ((absolute_start as i128 - loop_start_tick as i128)
            .rem_euclid(loop_len_ticks as i128)) as u64;
        let duration = note.end.saturating_sub(note.start).max(1);
        let end = relative_start
            .saturating_add(duration)
            .min(loop_len_ticks)
            .max(relative_start.saturating_add(1).min(loop_len_ticks));
        folded.add_event(MidiEvent::note_on(
            relative_start,
            note.channel,
            note.pitch,
            note.velocity.max(1),
        ));
        folded.add_event(MidiEvent::note_off(
            end,
            note.channel,
            note.pitch,
            0,
        ));
    }
    folded
}

/// Quantize a freshly recorded MIDI clip to a grid, preserving durations and
/// extending the clip if a shifted note ends past the original take.
fn quantize_recorded_midi_clip(clip: &mut MidiClip, grid_ticks: u64) {
    if grid_ticks <= 1 {
        return;
    }
    let mut notes = midi_clip_content_notes(clip);
    if notes.is_empty() {
        return;
    }
    quantize_midi_notes(&mut notes, grid_ticks, 1.0, 0.0);
    if let Some(end) = notes.iter().map(|note| note.end).max() {
        clip.length = clip.length.max(end);
    }
    write_piano_roll_notes(clip, &notes);
}

/// Transpose notes by a fixed number of semitones, clamping to the MIDI range.
fn transpose_midi_notes(notes: &mut [PianoRollNote], semitones: i32) {
    if semitones == 0 {
        return;
    }
    for note in notes {
        note.pitch = (note.pitch as i32 + semitones).clamp(0, 127) as u8;
    }
}

/// Scale note velocities by a factor, keeping them in the audible MIDI range.
fn scale_midi_velocities(notes: &mut [PianoRollNote], factor: f32) {
    if (factor - 1.0).abs() < 1e-4 {
        return;
    }
    for note in notes {
        note.velocity = (note.velocity as f32 * factor).round().clamp(1.0, 127.0) as u8;
    }
}

/// Pull velocities toward the middle of the MIDI range. `amount` is 0..1.
fn compress_midi_velocities(notes: &mut [PianoRollNote], amount: f32) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    for note in notes {
        let velocity = 64.0 + (note.velocity as f32 - 64.0) * (1.0 - amount);
        note.velocity = velocity.round().clamp(1.0, 127.0) as u8;
    }
}

/// Push velocities away from the middle of the MIDI range. `amount` is 0..1.
fn expand_midi_velocities(notes: &mut [PianoRollNote], amount: f32) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    for note in notes {
        let velocity = 64.0 + (note.velocity as f32 - 64.0) * (1.0 + amount);
        note.velocity = velocity.round().clamp(1.0, 127.0) as u8;
    }
}

/// Shape velocities with a gamma curve. Negative values darken soft notes;
/// positive values brighten them.
fn curve_midi_velocities(notes: &mut [PianoRollNote], curve: f32) {
    let curve = curve.clamp(-1.0, 1.0);
    if curve.abs() < 1e-4 {
        return;
    }
    let exponent = 2f32.powf(-2.0 * curve);
    for note in notes {
        let normalized = note.velocity as f32 / 127.0;
        note.velocity = (normalized.powf(exponent) * 127.0)
            .round()
            .clamp(1.0, 127.0) as u8;
    }
}

/// Small deterministic LCG used by the humanize tool.
fn humanize_random(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Add random timing and velocity offsets while preserving note durations.
fn humanize_midi_notes(
    notes: &mut [PianoRollNote],
    timing_ticks: u64,
    velocity_amount: u8,
    seed: u64,
) {
    if timing_ticks == 0 && velocity_amount == 0 {
        return;
    }
    let mut state = seed;
    for note in notes {
        let duration = note.end.saturating_sub(note.start).max(1);
        if timing_ticks > 0 {
            let jitter = humanize_random(&mut state) * 2.0 - 1.0;
            let offset = (jitter * timing_ticks as f32).round() as i64;
            let start = (note.start as i64 + offset).max(0) as u64;
            note.start = start;
            note.end = start.saturating_add(duration);
        }
        if velocity_amount > 0 {
            let jitter = humanize_random(&mut state) * 2.0 - 1.0;
            let offset = (jitter * velocity_amount as f32).round() as i32;
            note.velocity = (note.velocity as i32 + offset).clamp(1, 127) as u8;
        }
    }
}

/// Extend each note toward the next note start, leaving an optional gap.
fn legato_midi_notes(notes: &mut [PianoRollNote], gap_ticks: u64) {
    let starts: Vec<u64> = notes.iter().map(|note| note.start).collect();
    for note in notes.iter_mut() {
        if let Some(next) = starts
            .iter()
            .copied()
            .filter(|start| *start > note.start)
            .min()
        {
            let target = next.saturating_sub(gap_ticks);
            if target > note.start {
                note.end = note.end.max(target);
            }
        }
    }
}

/// Give every note the same duration.
fn set_midi_note_length(notes: &mut [PianoRollNote], length: u64) {
    let length = length.max(1);
    for note in notes {
        note.end = note.start.saturating_add(length);
    }
}

/// Reverse note positions within a span, preserving each note's duration.
fn reverse_midi_notes(notes: &mut [PianoRollNote], span_ticks: u64) {
    let span = span_ticks.max(1);
    for note in notes {
        let duration = note.end.saturating_sub(note.start).max(1);
        let start = span.saturating_sub(note.end);
        note.start = start;
        note.end = (start.saturating_add(duration))
            .min(span)
            .max(start + 1);
    }
}

/// Reflect pitches around a center note.
fn mirror_midi_notes(notes: &mut [PianoRollNote], center: u8) {
    let center = center as i16;
    for note in notes {
        note.pitch = (2 * center - note.pitch as i16).clamp(0, 127) as u8;
    }
}

/// Randomize pitch, velocity and timing while preserving note durations.
fn randomize_midi_notes(
    notes: &mut [PianoRollNote],
    pitch_amount: u8,
    velocity_amount: u8,
    timing_ticks: u64,
    seed: u64,
) {
    if pitch_amount == 0 && velocity_amount == 0 && timing_ticks == 0 {
        return;
    }
    let mut state = seed;
    for note in notes {
        let duration = note.end.saturating_sub(note.start).max(1);
        if pitch_amount > 0 {
            let jitter = humanize_random(&mut state) * 2.0 - 1.0;
            let offset = (jitter * pitch_amount as f32).round() as i16;
            note.pitch = (note.pitch as i16 + offset).clamp(0, 127) as u8;
        }
        if velocity_amount > 0 {
            let jitter = humanize_random(&mut state) * 2.0 - 1.0;
            let offset = (jitter * velocity_amount as f32).round() as i32;
            note.velocity = (note.velocity as i32 + offset).clamp(1, 127) as u8;
        }
        if timing_ticks > 0 {
            let jitter = humanize_random(&mut state) * 2.0 - 1.0;
            let offset = (jitter * timing_ticks as f32).round() as i64;
            let start = (note.start as i64 + offset).max(0) as u64;
            note.start = start;
            note.end = start.saturating_add(duration);
        }
    }
}

/// Join one note with the next same-pitch/channel note when the gap is small
/// enough. Returns whether a join happened.
fn join_note_with_next(
    notes: &mut Vec<PianoRollNote>,
    index: usize,
    max_gap: u64,
) -> bool {
    let Some(target) = notes.get(index).cloned() else {
        return false;
    };
    let mut next: Option<usize> = None;
    for (candidate_index, candidate) in notes.iter().enumerate() {
        if candidate_index == index
            || candidate.pitch != target.pitch
            || candidate.channel != target.channel
            || candidate.start < target.end
        {
            continue;
        }
        if candidate.start.saturating_sub(target.end) > max_gap {
            continue;
        }
        let better = next
            .map(|current| candidate.start < notes[current].start)
            .unwrap_or(true);
        if better {
            next = Some(candidate_index);
        }
    }
    let Some(next_index) = next else {
        return false;
    };
    let next_end = notes[next_index].end;
    notes[index].end = notes[index].end.max(next_end);
    notes.remove(next_index);
    true
}

/// Merge adjacent notes with the same pitch and channel. `max_gap` is the
/// largest silence that may remain inside a joined note.
fn join_midi_notes(notes: &mut Vec<PianoRollNote>, max_gap: u64) {
    if notes.len() < 2 {
        return;
    }
    notes.sort_by_key(|note| (note.pitch, note.channel, note.start, note.end));
    let mut joined: Vec<PianoRollNote> = Vec::with_capacity(notes.len());
    for note in notes.drain(..) {
        if let Some(last) = joined.last_mut() {
            if last.pitch == note.pitch
                && last.channel == note.channel
                && note.start <= last.end.saturating_add(max_gap)
            {
                last.end = last.end.max(note.end);
                continue;
            }
        }
        joined.push(note);
    }
    joined.sort_by_key(|note| (note.start, note.pitch, note.channel));
    *notes = joined;
}

/// Split every note that crosses `tick` into two notes. Notes that start or
/// end exactly on the tick are left alone.
fn split_midi_notes_at_tick(notes: &mut Vec<PianoRollNote>, tick: u64) {
    let mut split_notes = Vec::new();
    for note in notes.iter_mut() {
        if note.start < tick && tick < note.end {
            let end = note.end;
            note.end = tick;
            split_notes.push(PianoRollNote {
                start: tick,
                end,
                pitch: note.pitch,
                velocity: note.velocity,
                channel: note.channel,
            });
        }
    }
    if split_notes.is_empty() {
        return;
    }
    notes.extend(split_notes);
    notes.sort_by_key(|note| (note.start, note.pitch, note.channel));
}

/// Remove notes randomly while preserving the rest. `keep` is the probability
/// that a note survives, from 0 to 1.
fn thin_midi_notes(notes: &mut Vec<PianoRollNote>, keep: f32, seed: u64) {
    let keep = keep.clamp(0.0, 1.0);
    if keep >= 1.0 {
        return;
    }
    let mut state = seed;
    notes.retain(|_| humanize_random(&mut state) < keep);
}

/// Build the dimmed notes from other MIDI clips in the project, mapped into the
/// current clip's content coordinates. This mirrors FL Studio's ghost channels
/// and out-of-pattern ghost notes: other instruments/tracks are visible as
/// timing references while editing.
fn ghost_notes_for_clip(
    project: &Project,
    clip_id: ClipId,
    length: u64,
) -> Vec<PianoRollNote> {
    let Some(clip) = project.midi_clips.get(clip_id) else {
        return Vec::new();
    };
    let mut ghosts = Vec::new();
    for track_id in &project.track_order {
        let Some(track) = project.tracks.get(track_id) else {
            continue;
        };
        if !track.show_as_ghost {
            continue;
        }
        for other_id in &track.midi_clips {
            if *other_id == clip_id {
                continue;
            }
            let Some(other) = project.midi_clips.get(*other_id) else {
                continue;
            };
            for note in midi_clip_content_notes(other) {
                let timeline_tick = other.timeline_position.tick as i64
                    + note.start as i64
                    - other.content_offset as i64;
                let content_tick = timeline_tick
                    - clip.timeline_position.tick as i64
                    + clip.content_offset as i64;
                let duration = note.end.saturating_sub(note.start).max(1);
                let end_tick = content_tick + duration as i64;
                if end_tick <= 0 || content_tick >= length as i64 {
                    continue;
                }
                let start = content_tick.max(0) as u64;
                let end = end_tick.max(start as i64 + 1) as u64;
                ghosts.push(PianoRollNote {
                    start,
                    end,
                    ..note
                });
            }
        }
    }
    ghosts
}

/// Snap resolution in ticks used for MIDI clip trimming in the arrange view.
fn timeline_snap_ticks(snap: SnapMode, beats_per_bar: u64) -> u64 {
    match snap {
        SnapMode::Off => 1,
        SnapMode::Bar => 960 * beats_per_bar.max(1),
        SnapMode::Beat => 960,
        SnapMode::Eighth => 480,
        SnapMode::Sixteenth => 240,
    }
}

/// Compute the visible start/offset/length after dragging a MIDI clip's left
/// edge. Trimming moves the visible window over the content; events are never
/// deleted.
fn midi_trim_start(
    before: &MidiClip,
    requested_tick: u64,
    snap_ticks: u64,
) -> (u64, u64, u64) {
    let snap = |tick: u64| -> u64 {
        if snap_ticks <= 1 {
            tick
        } else {
            ((tick + snap_ticks / 2) / snap_ticks) * snap_ticks
        }
    };
    let earliest_start = before
        .timeline_position
        .tick
        .saturating_sub(before.content_offset);
    let max_start = before.timeline_position.tick + before.length.saturating_sub(1);
    let new_start = snap(requested_tick).clamp(earliest_start, max_start);
    let delta = new_start as i64 - before.timeline_position.tick as i64;
    let new_offset = (before.content_offset as i64 + delta).max(0) as u64;
    let new_length = (before.length as i64 - delta).max(1) as u64;
    (new_start, new_offset, new_length)
}

/// Compute the new visible length after dragging a MIDI clip's right edge.
fn midi_trim_end(before: &MidiClip, requested_tick: u64, snap_ticks: u64) -> u64 {
    let snap = |tick: u64| -> u64 {
        if snap_ticks <= 1 {
            tick
        } else {
            ((tick + snap_ticks / 2) / snap_ticks) * snap_ticks
        }
    };
    let min_end = before.timeline_position.tick + 1;
    let new_end = snap(requested_tick).max(min_end);
    new_end - before.timeline_position.tick
}

/// Split a MIDI clip at a timeline tick. The two halves keep the original
/// content and share it through their visible windows, so splitting is
/// non-destructive.
fn split_midi_clip_at_tick(clip: &MidiClip, split_tick: u64) -> Option<(MidiClip, MidiClip)> {
    let offset = split_tick.checked_sub(clip.timeline_position.tick)?;
    if offset == 0 || offset >= clip.length {
        return None;
    }

    let mut left = clip.clone();
    left.length = offset;

    let mut right = clip.clone();
    right.id = ClipId::default();
    right.timeline_position.tick = split_tick;
    right.content_offset = clip.content_offset.saturating_add(offset);
    right.length = clip.length - offset;
    Some((left, right))
}

/// Fit the piano-roll view to the clip length and the notes' pitch range.
fn fit_piano_roll_view(window: &mut PianoRollWindow, grid: egui::Rect, length: u64) {
    let length = length.max(1) as f32;
    window.pixels_per_tick = (grid.width() / length).clamp(0.0005, 2.0);
    window.view_start_tick = 0.0;

    let (min, max) = if window.notes.is_empty() {
        (48u8, 72u8)
    } else {
        (
            window.notes.iter().map(|note| note.pitch).min().unwrap_or(48),
            window.notes.iter().map(|note| note.pitch).max().unwrap_or(72),
        )
    };
    let low = min.saturating_sub(2);
    let high = max.saturating_add(2).min(127);
    let span = high.saturating_sub(low).saturating_add(1).max(1) as f32;
    let pixels_per_semitone = (grid.height() / span).clamp(4.0, 24.0);
    window.pixels_per_semitone = pixels_per_semitone;

    let visible = (grid.height() / pixels_per_semitone).max(1.0);
    let center = (low as f32 + high as f32) * 0.5;
    window.view_low_pitch =
        (center - visible * 0.5).clamp(0.0, (128.0 - visible).max(0.0));
    window.needs_fit = false;
}

/// Fit the piano-roll view to the current note selection.
fn fit_piano_roll_selection(window: &mut PianoRollWindow, grid: egui::Rect) {
    let mut min_start = u64::MAX;
    let mut max_end = 0u64;
    let mut min_pitch = 127u8;
    let mut max_pitch = 0u8;
    let mut count = 0usize;
    for index in &window.selected {
        if let Some(note) = window.notes.get(*index) {
            min_start = min_start.min(note.start);
            max_end = max_end.max(note.end);
            min_pitch = min_pitch.min(note.pitch);
            max_pitch = max_pitch.max(note.pitch);
            count += 1;
        }
    }
    if count == 0 {
        return;
    }

    let span_ticks = max_end.saturating_sub(min_start).max(1) as f32;
    let pad_ticks = (span_ticks * 0.05).max(1.0);
    window.pixels_per_tick =
        (grid.width() / (span_ticks + pad_ticks * 2.0)).clamp(0.0005, 4.0);
    window.view_start_tick = (min_start as f64 - pad_ticks as f64).max(0.0);

    let span_pitches = (max_pitch.saturating_sub(min_pitch) + 1).max(1) as f32;
    window.pixels_per_semitone =
        (grid.height() / (span_pitches + 2.0)).clamp(4.0, 24.0);
    let visible = (grid.height() / window.pixels_per_semitone).max(1.0);
    let center = (min_pitch as f32 + max_pitch as f32) * 0.5;
    window.view_low_pitch =
        (center - visible * 0.5).clamp(0.0, (128.0 - visible).max(0.0));
    window.needs_fit = false;
}

/// Convert a loop range in seconds to output-device frames. Returns `None`
/// when looping is off or the range is empty.
fn loop_range_frames(
    enabled: bool,
    start_secs: f64,
    end_secs: f64,
    out_rate: u32,
) -> Option<(u64, u64)> {
    if !enabled || end_secs <= start_secs {
        return None;
    }
    let rate = out_rate.max(1) as f64;
    let start = (start_secs.max(0.0) * rate) as u64;
    let end = (end_secs.max(0.0) * rate).ceil() as u64;
    if end > start {
        Some((start, end))
    } else {
        None
    }
}

/// Build metronome settings from the project tempo and time signature.
fn metronome_config(
    enabled: bool,
    bpm: f32,
    numerator: u8,
    denominator: u8,
    out_rate: u32,
) -> Option<MetronomeConfig> {
    if !enabled {
        return None;
    }
    let bpm = bpm.max(1.0) as f64;
    let beat_secs = 60.0 / bpm * (4.0 / denominator.max(1) as f64);
    Some(MetronomeConfig {
        frames_per_beat: beat_secs * out_rate.max(1) as f64,
        beats_per_bar: numerator.max(1) as u32,
        gain: 0.5,
    })
}

/// Average BPM from tap intervals in seconds. Returns `None` when there are no
/// taps. Extreme values are clamped to a musical range.
fn bpm_from_tap_intervals(intervals: &[f64]) -> Option<f32> {
    if intervals.is_empty() {
        return None;
    }
    let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
    if average <= 0.0 {
        return None;
    }
    Some((60.0 / average).clamp(20.0, 300.0) as f32)
}

/// Whether an armed take should wait for the punch-in point instead of
/// capturing immediately.
fn punch_waiting(punch_enabled: bool, punch_in: f64, punch_out: f64, playhead: f64) -> bool {
    punch_enabled && punch_out > punch_in && playhead < punch_in
}

/// Small pre-fader recording meter: a green RMS bar with a peak tick.
fn draw_input_meter(ui: &mut egui::Ui, peak: f32, rms: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(30));

    let rms_width = rect.width() * rms.clamp(0.0, 1.0);
    if rms_width > 0.0 {
        let bar = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rms_width, rect.bottom()),
        );
        painter.rect_filled(bar, 2.0, egui::Color32::from_rgb(70, 180, 90));
    }

    let peak_x = rect.left() + rect.width() * peak.clamp(0.0, 1.0);
    if peak > 0.0 {
        let color = if peak >= 0.99 {
            egui::Color32::RED
        } else {
            egui::Color32::from_rgb(220, 200, 80)
        };
        painter.line_segment(
            [
                egui::pos2(peak_x, rect.top()),
                egui::pos2(peak_x, rect.bottom()),
            ],
            egui::Stroke::new(2.0_f32, color),
        );
    }

    let db = 20.0 * peak.max(1e-6).log10();
    ui.label(format!("{:.0} dB", db));
}

/// Number of wheel notches over a hovered slider. Positive means increase.
fn slider_wheel_steps(ui: &egui::Ui) -> f32 {
    if ui.input(|i| i.modifiers.command || i.modifiers.ctrl) {
        return 0.0;
    }
    let mut steps = 0.0;
    ui.input(|i| {
        for event in &i.events {
            if let egui::Event::MouseWheel { delta, .. } = event {
                if delta.y != 0.0 {
                    steps += delta.y.signum();
                }
            }
        }
    });
    steps.clamp(-1.0, 1.0)
}

/// Move a slider value by `steps` notches, where each notch is 5% of the
/// slider's full range.
fn slider_wheel_value(current: f64, start: f64, end: f64, steps: f32) -> f64 {
    let step = (end - start) * 0.05;
    (current + step * steps as f64).clamp(start, end)
}

/// Move a slider value by a caller-provided step per wheel notch.
fn slider_wheel_value_with_step(
    current: f64,
    start: f64,
    end: f64,
    steps: f32,
    step: f64,
) -> f64 {
    (current + step * steps as f64).clamp(start, end)
}

/// Move a logarithmic slider value by 5% of its logarithmic travel per notch.
fn slider_wheel_log_value(
    current: f64,
    start: f64,
    end: f64,
    steps: f32,
) -> f64 {
    if start <= 0.0 || end <= start || current <= 0.0 {
        return current.clamp(start, end);
    }
    let log_step = (end.log10() - start.log10()) * 0.05;
    (current * 10f64.powf(log_step * steps as f64)).clamp(start, end)
}

/// Apply a wheel adjustment before a slider is drawn. The slider's previous
/// rectangle is stored under its auto-id so the wheel only affects the slider
/// under the pointer.
fn apply_slider_wheel<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: &std::ops::RangeInclusive<Num>,
    id: egui::Id,
    wheel_step: Option<f64>,
    logarithmic: bool,
) -> bool {
    let Some(rect) = ui.data(|data| data.get_temp::<egui::Rect>(id)) else {
        return false;
    };
    if !ui.rect_contains_pointer(rect) {
        return false;
    }
    let steps = slider_wheel_steps(ui);
    if steps == 0.0 {
        return false;
    }
    // Consume the scroll even when throttled so the surrounding scroll area
    // does not move while the pointer is over a slider.
    ui.input_mut(|i| {
        i.raw_scroll_delta = egui::Vec2::ZERO;
        i.smooth_scroll_delta = egui::Vec2::ZERO;
    });
    let now = ui.input(|i| i.time);
    let last_id = id.with("wheel_time");
    let last = ui
        .data(|data| data.get_temp::<f64>(last_id))
        .unwrap_or(f64::NEG_INFINITY);
    if now - last < 0.05 {
        return false;
    }
    ui.data_mut(|data| data.insert_temp(last_id, now));

    let start = range.start().to_f64();
    let end = range.end().to_f64();
    let mut next = if logarithmic {
        slider_wheel_log_value(value.to_f64(), start, end, steps)
    } else if let Some(step) = wheel_step {
        slider_wheel_value_with_step(
            value.to_f64(),
            start,
            end,
            steps,
            step,
        )
    } else {
        slider_wheel_value(value.to_f64(), start, end, steps)
    };
    if Num::INTEGRAL {
        next = next.round();
    }
    *value = Num::from_f64(next);
    true
}

/// Add a slider that supports wheel adjustment in 5% range steps.
fn add_wheel_slider<'a, Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &'a mut Num,
    range: std::ops::RangeInclusive<Num>,
    build: impl FnOnce(egui::Slider<'a>) -> egui::Slider<'a>,
) -> egui::Response {
    add_wheel_slider_step(ui, value, range, None, false, build)
}

/// Add a slider with an explicit wheel step.
fn add_wheel_slider_step<'a, Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &'a mut Num,
    range: std::ops::RangeInclusive<Num>,
    wheel_step: Option<f64>,
    logarithmic: bool,
    build: impl FnOnce(egui::Slider<'a>) -> egui::Slider<'a>,
) -> egui::Response {
    let id = ui.next_auto_id();
    let wheel_changed =
        apply_slider_wheel(ui, value, &range, id, wheel_step, logarithmic);
    let mut response = ui.add(build(egui::Slider::new(value, range)));
    if wheel_changed {
        response.mark_changed();
    }
    ui.data_mut(|data| data.insert_temp(id, response.rect));
    response
}

/// Add a fixed-size slider with an explicit wheel step.
fn add_sized_wheel_slider_step<'a, Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    value: &'a mut Num,
    range: std::ops::RangeInclusive<Num>,
    wheel_step: Option<f64>,
    logarithmic: bool,
    build: impl FnOnce(egui::Slider<'a>) -> egui::Slider<'a>,
) -> egui::Response {
    let id = ui.next_auto_id();
    let wheel_changed =
        apply_slider_wheel(ui, value, &range, id, wheel_step, logarithmic);
    let mut response =
        ui.add_sized(size, build(egui::Slider::new(value, range)));
    if wheel_changed {
        response.mark_changed();
    }
    ui.data_mut(|data| data.insert_temp(id, response.rect));
    response
}

/// Horizontal peak meter used by the FX Rack channel strips.
fn draw_track_meter(ui: &mut egui::Ui, peak: f32, hold: f32) {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, 8.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(24));
    let level = peak.clamp(0.0, 1.0);
    if level > 0.0 {
        let filled = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * level, rect.bottom()),
        );
        let color = if level >= 0.99 {
            egui::Color32::RED
        } else if level > 0.8 {
            egui::Color32::from_rgb(220, 200, 80)
        } else {
            egui::Color32::from_rgb(70, 180, 90)
        };
        painter.rect_filled(filled, 2.0, color);
    }
    let hold = hold.clamp(0.0, 1.0);
    if hold > 0.01 {
        let x = rect.left() + rect.width() * hold;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
        );
    }
    let db = 20.0 * peak.max(1e-6).log10();
    let hold_db = 20.0 * hold.max(1e-6).log10();
    response.on_hover_text(format!(
        "Peak: {:.1} dB · Hold: {:.1} dB",
        db, hold_db
    ));
}

/// Stereo master meter shown in the FX Rack header.
fn draw_master_meter(ui: &mut egui::Ui, levels: [f32; 2]) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("Master").small());
        for (label, level) in [("L", levels[0]), ("R", levels[1])] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).small());
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(120.0, 6.0),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                painter.rect_filled(rect, 2.0, egui::Color32::from_gray(24));
                let level = level.clamp(0.0, 1.0);
                if level > 0.0 {
                    let filled = egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(
                            rect.left() + rect.width() * level,
                            rect.bottom(),
                        ),
                    );
                    let color = if level >= 0.99 {
                        egui::Color32::RED
                    } else if level > 0.8 {
                        egui::Color32::from_rgb(220, 200, 80)
                    } else {
                        egui::Color32::from_rgb(70, 180, 90)
                    };
                    painter.rect_filled(filled, 2.0, color);
                }
                let db = 20.0 * level.max(1e-6).log10();
                response.on_hover_text(format!("{}: {:.1} dB", label, db));
            });
        }
    });
}

/// Source reference and trimmed range, used by processing operations that
/// render a new file (the original file is never modified).
#[derive(Debug, Clone)]
struct ClipSourceState {
    source: AudioFileRef,
    source_offset: u64,
    length: u64,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
    gain: f32,
}

impl ClipSourceState {
    fn of(clip: &AudioClip) -> Self {
        Self {
            source: clip.source.clone(),
            source_offset: clip.source_offset,
            length: clip.length,
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
            gain: clip.gain,
        }
    }

    fn apply(&self, clip: &mut AudioClip) {
        clip.source = self.source.clone();
        clip.source_offset = self.source_offset;
        clip.length = self.length;
        clip.fade_in = self.fade_in;
        clip.fade_out = self.fade_out;
        clip.gain = self.gain;
    }
}

/// Clamp a group move so the earliest clip cannot pass the timeline start.
fn clamp_group_delta(min_start: u64, raw_delta: i64) -> i64 {
    raw_delta.max(-(min_start as i64))
}

/// Project-sample span `(start, end)` of a clip's audible range.
fn clip_span_samples(clip: &AudioClip, project_sr: f64) -> (u64, u64) {
    let start = clip.timeline_position.sample;
    let length = (clip.length as f64 * project_sr.max(1.0)
        / clip.source.sample_rate.max(1) as f64)
        .ceil() as u64;
    (start, start.saturating_add(length))
}

/// Assign sub-lanes to clips that overlap significantly, so stacked takes can
/// be told apart. Returns `(index, count)` per clip; clips with no significant
/// overlap are `(0, 1)`.
fn take_lane_layout(project: &Project, clips: &[ClipId]) -> HashMap<ClipId, (usize, usize)> {
    let project_sr = project.sample_rate.max(1) as f64;
    let mut spans: Vec<(ClipId, u64, u64)> = clips
        .iter()
        .filter_map(|id| {
            let clip = project.audio_clips.get(*id)?;
            if clip.length == 0 {
                return None;
            }
            let (start, end) = clip_span_samples(clip, project_sr);
            Some((*id, start, end))
        })
        .collect();
    spans.sort_by_key(|(_, start, end)| (*start, *end));

    fn significant(a: (u64, u64), b: (u64, u64)) -> bool {
        let start = a.0.max(b.0);
        let end = a.1.min(b.1);
        if end <= start {
            return false;
        }
        let overlap = end - start;
        let min_length = (a.1 - a.0).min(b.1 - b.0).max(1);
        overlap.saturating_mul(4) >= min_length
    }

    fn flush(
        cluster: &mut Vec<(ClipId, u64, u64)>,
        layout: &mut HashMap<ClipId, (usize, usize)>,
    ) {
        if cluster.len() > 1 {
            let count = cluster.len();
            for (index, (id, _, _)) in cluster.iter().enumerate() {
                layout.insert(*id, (index, count));
            }
        }
        cluster.clear();
    }

    let mut layout = HashMap::new();
    let mut cluster: Vec<(ClipId, u64, u64)> = Vec::new();
    for span in spans {
        if !cluster.is_empty()
            && !cluster
                .iter()
                .any(|member| significant((member.1, member.2), (span.1, span.2)))
        {
            flush(&mut cluster, &mut layout);
        }
        cluster.push(span);
    }
    flush(&mut cluster, &mut layout);
    layout
}

/// IDs of clips whose screen rectangles intersect the marquee.
fn clips_in_marquee(clip_rects: &[(ClipId, egui::Rect)], marquee: egui::Rect) -> Vec<ClipId> {
    clip_rects
        .iter()
        .filter(|(_, rect)| rect.intersects(marquee))
        .map(|(id, _)| *id)
        .collect()
}

fn midi_clips_in_marquee(
    clip_rects: &[(ClipId, egui::Rect)],
    marquee: egui::Rect,
) -> Vec<ClipId> {
    clip_rects
        .iter()
        .filter(|(_, rect)| rect.intersects(marquee))
        .map(|(id, _)| *id)
        .collect()
}

/// Clips on one track that touch or overlap the selected clip. Consolidation
/// renders this whole chain into one file.
#[derive(Debug, Clone)]
struct ConsolidationPlan {
    track: TrackId,
    clip_ids: Vec<ClipId>,
    start: u64,
    end: u64,
}

/// Expand a sorted list of `(clip, start, end)` spans around `selected`. A run
/// grows while clips touch or overlap.
fn contiguous_clip_span(
    spans: &[(ClipId, u64, u64)],
    selected: ClipId,
) -> Option<(Vec<ClipId>, u64, u64)> {
    let index = spans.iter().position(|(id, _, _)| *id == selected)?;
    let mut first = index;
    let mut last = index;
    let mut start = spans[index].1;
    let mut end = spans[index].2;

    while first > 0 && spans[first - 1].2 >= start {
        first -= 1;
        start = start.min(spans[first].1);
        end = end.max(spans[first].2);
    }
    while last + 1 < spans.len() && spans[last + 1].1 <= end {
        last += 1;
        start = start.min(spans[last].1);
        end = end.max(spans[last].2);
    }

    Some((
        spans[first..=last]
            .iter()
            .map(|(id, _, _)| *id)
            .collect(),
        start,
        end,
    ))
}

/// One undoable edit. Structural commands reference clips by id (the clip data
/// stays in the project pool), so history entries are cheap.
#[derive(Debug, Clone)]
enum EditAction {
    /// Move a clip to another timeline position and/or track.
    Move {
        clip_id: ClipId,
        from_track: TrackId,
        from_pos: TimelinePos,
        to_track: TrackId,
        to_pos: TimelinePos,
    },
    /// Non-destructive change: trim, fade, gain or color.
    Shape {
        clip_id: ClipId,
        from: ClipShape,
        to: ClipShape,
    },
    /// Replace the clip's source file and trimmed range (offline processing).
    Source {
        clip_id: ClipId,
        from: ClipSourceState,
        to: ClipSourceState,
    },
    /// Place an existing clip on a track (undo takes it off).
    Attach { track: TrackId, clip_id: ClipId },
    /// Take a clip off its track (undo puts it back). The audio and clip data
    /// remain in the project, so deleted clips are recoverable.
    Detach { track: TrackId, clip_id: ClipId },
    /// Change the project time signature.
    TimeSignature { from: (u8, u8), to: (u8, u8) },
    /// Change the project tempo (BPM).
    Tempo { from: f32, to: f32 },
    /// Toggle track mute, solo or record-arm.
    TrackState {
        track_id: TrackId,
        from: TrackShape,
        to: TrackShape,
    },
    /// Replace a track's real-time insert chain.
    TrackInserts {
        track_id: TrackId,
        from: Vec<InsertSlot>,
        to: Vec<InsertSlot>,
    },
    /// Add a new track (undo removes it again).
    AddTrack { track: Track },
    /// Remove a track, remembering its original order for undo.
    RemoveTrack { track: Track, index: usize },
    /// Reorder a track in the track list.
    MoveTrack {
        track_id: TrackId,
        from_index: usize,
        to_index: usize,
    },
    /// Attach a MIDI clip to a track (undo detaches it; data stays in the pool).
    AttachMidi { track: TrackId, clip_id: ClipId },
    /// Detach a MIDI clip from its track (undo reattaches).
    DetachMidi { track: TrackId, clip_id: ClipId },
    /// Move a MIDI clip in time and/or to another MIDI/instrument track.
    MoveMidi {
        clip_id: ClipId,
        from_track: TrackId,
        from_pos: TimelinePos,
        to_track: TrackId,
        to_pos: TimelinePos,
    },
    /// Replace a MIDI clip's notes (piano-roll editing).
    MidiClip {
        clip_id: ClipId,
        from: MidiClip,
        to: MidiClip,
    },
    /// Add a marker region to the ruler.
    AddMarker { marker: MarkerRegion },
    /// Remove a marker region from the ruler.
    RemoveMarker { marker: MarkerRegion },
    /// Several edits applied and undone as one history step.
    Group {
        actions: Vec<EditAction>,
        label: String,
    },
}

impl EditAction {
    /// Human-readable label shown in the History panel.
    fn description(&self) -> String {
        match self {
            EditAction::Move { .. } => "Move clip".to_string(),
            EditAction::Shape { from, to, .. } => {
                if from.muted != to.muted {
                    if to.muted { "Mute take" } else { "Unmute take" }.to_string()
                } else if from.gain != to.gain {
                    "Clip gain".to_string()
                } else if from.fade_in != to.fade_in {
                    "Fade in".to_string()
                } else if from.fade_out != to.fade_out {
                    "Fade out".to_string()
                } else if from.color != to.color {
                    "Clip color".to_string()
                } else if from.position != to.position
                    || from.source_offset != to.source_offset
                    || from.length != to.length
                {
                    "Trim clip".to_string()
                } else {
                    "Edit clip".to_string()
                }
            }
            EditAction::Source { from, to, .. } => {
                if from.source.path != to.source.path {
                    "Process clip".to_string()
                } else {
                    "Edit clip source".to_string()
                }
            }
            EditAction::Attach { .. } => "Add clip".to_string(),
            EditAction::Detach { .. } => "Delete clip".to_string(),
            EditAction::TimeSignature { to, .. } => {
                format!("Time signature {}/{}", to.0, to.1)
            }
            EditAction::Tempo { to, .. } => format!("Tempo {:.1} BPM", to),
            EditAction::TrackState { from, to, .. } => {
                if from.mute != to.mute {                    if to.mute { "Mute track" } else { "Unmute track" }.to_string()
                } else if from.solo != to.solo {
                    if to.solo { "Solo track" } else { "Unsolo track" }.to_string()
                } else if from.record_arm != to.record_arm {
                    if to.record_arm {
                        "Arm track"
                    } else {
                        "Disarm track"
                    }
                    .to_string()
                } else if from.volume != to.volume {
                    "Track volume".to_string()
                } else if from.pan != to.pan {
                    "Track pan".to_string()
                } else if from.monitor_mode != to.monitor_mode {
                    match to.monitor_mode {
                        MonitorMode::Off => "Monitor off",
                        MonitorMode::Auto => "Monitor auto",
                        MonitorMode::On => "Monitor on",
                    }
                    .to_string()
                } else {
                    "Track change".to_string()
                }
            }
            EditAction::AddTrack { track } => format!("Add track: {}", track.name),
            EditAction::RemoveTrack { track, .. } => format!("Remove track: {}", track.name),
            EditAction::MoveTrack { .. } => "Reorder track".to_string(),
            EditAction::TrackInserts { from, to, .. } => {
                if to.len() > from.len() {
                    "Add track effect".to_string()
                } else if to.len() < from.len() {
                    "Remove track effect".to_string()
                } else {
                    "Track effects".to_string()
                }
            }
            EditAction::AttachMidi { .. } => "Add MIDI clip".to_string(),
            EditAction::DetachMidi { .. } => "Delete MIDI clip".to_string(),
            EditAction::MoveMidi { .. } => "Move MIDI clip".to_string(),
            EditAction::MidiClip { .. } => "Edit MIDI clip".to_string(),
            EditAction::AddMarker { marker } => {
                format!("Add marker: {}", marker.name)
            }
            EditAction::RemoveMarker { marker } => {
                format!("Remove marker: {}", marker.name)
            }
            EditAction::Group { label, .. } => label.clone(),
        }
    }
}

/// Merge a continuous `Shape` edit into the most recent history entry when the
/// new edit starts exactly where the previous one ended (same clip). Returns
/// true when the edit was merged.
fn merge_continuous_edit(history: &mut History<EditAction>, action: &EditAction) -> bool {
    let Some(entry) = history.last_mut() else {
        return false;
    };
    let mut merged = false;
    match action {
        EditAction::Shape { clip_id, from, to } => {
            let (clip_id, from, to) = (*clip_id, *from, *to);
            if let EditAction::Shape {
                clip_id: prev_id,
                to: prev_to,
                ..
            } = &mut entry.action
            {
                if *prev_id == clip_id && *prev_to == from {
                    *prev_to = to;
                    merged = true;
                }
            }
        }
        EditAction::TrackState {
            track_id,
            from,
            to,
        } => {
            if let EditAction::TrackState {
                track_id: prev_id,
                to: prev_to,
                ..
            } = &mut entry.action
            {
                if *prev_id == *track_id && *prev_to == *from {
                    *prev_to = *to;
                    merged = true;
                }
            }
        }
        _ => {}
    }
    if merged {
        entry.description = entry.action.description();
    }
    merged
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    TrimStart,
    TrimEnd,
    FadeIn,
    FadeOut,
}

/// Effect selected in the floating audio-effects window.
/// Tools in the Clip Audio window. The former offline Bit Crusher, Saturation,
/// Filter and Tremolo variants are retained for compatibility but are no
/// longer listed; real-time versions live in the FX Rack.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectKind {
    Normalize,
    LufsNormalize,
    BakeIntoClip,
    Reverse,
    PitchShift,
    TimeStretch,
    BitCrusher,
    Saturation,
    Filter,
    Tremolo,
    Consolidate,
}

impl EffectKind {
    const ALL: [Self; 7] = [
        Self::Normalize,
        Self::LufsNormalize,
        Self::BakeIntoClip,
        Self::Reverse,
        Self::PitchShift,
        Self::TimeStretch,
        Self::Consolidate,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Normalize => "Normalize",
            Self::LufsNormalize => "LUFS Normalize",
            Self::BakeIntoClip => "Bake into Clip",
            Self::Reverse => "Reverse",
            Self::PitchShift => "Pitch Shift",
            Self::TimeStretch => "Time Stretch",
            Self::BitCrusher => "Bit Crusher",
            Self::Saturation => "Saturation",
            Self::Filter => "Filter",
            Self::Tremolo => "Tremolo",
            Self::Consolidate => "Consolidate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterKind {
    LowPass,
    HighPass,
}

impl FilterKind {
    fn label(self) -> &'static str {
        match self {
            Self::LowPass => "Low-pass",
            Self::HighPass => "High-pass",
        }
    }
}

/// Kinds of built-in real-time insert available on every track type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertEffectKind {
    Saturation,
    Filter,
    BitCrusher,
    Tremolo,
}

impl InsertEffectKind {
    const ALL: [Self; 4] = [
        Self::Saturation,
        Self::Filter,
        Self::BitCrusher,
        Self::Tremolo,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Saturation => "Saturation",
            Self::Filter => "Filter",
            Self::BitCrusher => "Bit Crusher",
            Self::Tremolo => "Tremolo",
        }
    }

    fn default_effect(self) -> TrackEffect {
        match self {
            Self::Saturation => TrackEffect::default_saturation(),
            Self::Filter => TrackEffect::default_filter(),
            Self::BitCrusher => TrackEffect::default_bit_crusher(),
            Self::Tremolo => TrackEffect::default_tremolo(),
        }
    }

    fn from_effect(effect: &TrackEffect) -> Self {
        match effect {
            TrackEffect::Saturation { .. } => Self::Saturation,
            TrackEffect::Filter { .. } => Self::Filter,
            TrackEffect::BitCrusher { .. } => Self::BitCrusher,
            TrackEffect::Tremolo { .. } => Self::Tremolo,
        }
    }
}

/// State of the floating per-clip audio window: gain/fades/colour plus the
/// offline effects.
#[derive(Debug, Clone)]
struct ClipAudioWindow {
    clip_id: ClipId,
    /// Every clip the next effect Apply should process (primary first).
    clip_ids: Vec<ClipId>,
    /// Selected effect; `None` shows the clip's gain, fades and colour.
    selected: Option<EffectKind>,
    pitch: f32,
    stretch: f32,
    stretch_mode: TimeStretchMode,
    formants: bool,
    lufs_target: f32,
    crush_bits: f32,
    crush_downsample: f32,
    drive_db: f32,
    saturation_mix: f32,
    filter_kind: FilterKind,
    filter_cutoff: f32,
    tremolo_rate: f32,
    tremolo_depth: f32,
}

impl ClipAudioWindow {
    fn new(clip_ids: Vec<ClipId>) -> Self {
        let clip_id = clip_ids.first().copied().unwrap_or_default();
        Self {
            clip_id,
            clip_ids,
            selected: None,
            pitch: 0.0,
            stretch: 1.0,
            stretch_mode: TimeStretchMode::Elastic,
            formants: true,
            lufs_target: -14.0,
            crush_bits: 8.0,
            crush_downsample: 4.0,
            drive_db: 12.0,
            saturation_mix: 100.0,
            filter_kind: FilterKind::LowPass,
            filter_cutoff: 8_000.0,
            tremolo_rate: 5.0,
            tremolo_depth: 50.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoteDragMode {
    Move,
    ResizeEnd,
    Velocity,
}

/// Pointer tool selected in the piano-roll header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PianoRollTool {
    Select,
    Draw,
    Erase,
    Split,
    Glue,
    Velocity,
}

impl PianoRollTool {
    const ALL: [Self; 6] = [
        Self::Select,
        Self::Draw,
        Self::Erase,
        Self::Split,
        Self::Glue,
        Self::Velocity,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Draw => "Draw",
            Self::Erase => "Erase",
            Self::Split => "Split",
            Self::Glue => "Glue",
            Self::Velocity => "Vel",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Select => {
                "Select and drag notes; double-click empty space to add one"
            }
            Self::Draw => "Click empty space to add a note",
            Self::Erase => "Click a note to delete it",
            Self::Split => "Click a note to split it at that point",
            Self::Glue => {
                "Click a note to join it with the next same-pitch note"
            }
            Self::Velocity => {
                "Drag a note up or down to change its velocity"
            }
        }
    }
}

/// Snap resolutions available inside the piano roll. Finer than the arrange
/// grid so drum and bass parts can be placed precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PianoRollSnap {
    Off,
    Bar,
    Beat,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
}

impl PianoRollSnap {
    const ALL: [Self; 8] = [
        Self::Off,
        Self::Bar,
        Self::Beat,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::EighthTriplet,
        Self::SixteenthTriplet,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Bar => "Bar",
            Self::Beat => "Beat",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::EighthTriplet => "1/8T",
            Self::SixteenthTriplet => "1/16T",
        }
    }

    fn ticks(self, beats_per_bar: u64) -> u64 {
        match self {
            Self::Off => 1,
            Self::Bar => 960 * beats_per_bar.max(1),
            Self::Beat => 960,
            Self::Eighth => 480,
            Self::Sixteenth => 240,
            Self::ThirtySecond => 120,
            Self::EighthTriplet => 320,
            Self::SixteenthTriplet => 160,
        }
    }

    fn from_arrange(snap: SnapMode) -> Self {
        match snap {
            SnapMode::Off => Self::Off,
            SnapMode::Bar => Self::Bar,
            SnapMode::Beat => Self::Beat,
            SnapMode::Eighth => Self::Eighth,
            SnapMode::Sixteenth => Self::Sixteenth,
        }
    }
}

/// In-progress piano-roll note gesture.
#[derive(Debug, Clone)]
struct NoteDrag {
    mode: NoteDragMode,
    anchor_tick: u64,
    anchor_pitch: u8,
    /// Note indices and their values when the gesture started.
    originals: Vec<(usize, PianoRollNote)>,
    /// Last pitch auditioned during this drag, so moving across semitones
    /// retriggers the preview without spamming the synth every frame.
    preview_pitch: Option<u8>,
    /// Clip snapshot for one undo entry when the gesture ends.
    before: MidiClip,
}

/// State of the floating piano-roll window for one MIDI clip.
#[derive(Debug, Clone)]
struct PianoRollWindow {
    clip_id: ClipId,
    notes: Vec<PianoRollNote>,
    initialized: bool,
    selected: BTreeSet<usize>,
    drag: Option<NoteDrag>,
    /// Length used for the next newly added note.
    last_length: u64,
    /// First visible tick at the left edge of the grid.
    view_start_tick: f64,
    /// Horizontal zoom.
    pixels_per_tick: f32,
    /// Pitch at the bottom edge of the grid.
    view_low_pitch: f32,
    /// Vertical zoom.
    pixels_per_semitone: f32,
    /// Re-fit the view on the next frame.
    needs_fit: bool,
    /// Snap resolution chosen in the piano-roll header.
    snap: PianoRollSnap,
    /// Quantize/transpose/velocity controls shown in the left panel.
    tools: MidiToolsState,
    /// Rubber-band note selection.
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    marquee_additive: bool,
    /// Clip snapshot while dragging a velocity bar.
    velocity_drag_before: Option<MidiClip>,
    /// Visible control-change lanes, in ascending controller order.
    cc_lanes: Vec<u8>,
    show_velocity_lane: bool,
    show_ghost_notes: bool,
    /// Tint pitch rows that belong to the selected scale.
    show_scale_highlight: bool,
    /// Show one labelled note row per pitch instead of piano keys.
    drum_mode: bool,
    /// Active pointer tool.
    tool: PianoRollTool,
    /// Clip snapshot while drawing control-change points.
    cc_drag_before: Option<MidiClip>,
}

impl PianoRollWindow {
    fn new(clip_id: ClipId, snap: SnapMode) -> Self {
        Self {
            clip_id,
            notes: Vec::new(),
            initialized: false,
            selected: BTreeSet::new(),
            drag: None,
            last_length: 960,
            view_start_tick: 0.0,
            pixels_per_tick: 0.1,
            view_low_pitch: 48.0,
            pixels_per_semitone: 16.0,
            needs_fit: true,
            snap: PianoRollSnap::from_arrange(snap),
            tools: MidiToolsState::default(),
            marquee: None,
            marquee_additive: false,
            velocity_drag_before: None,
            cc_lanes: Vec::new(),
            show_velocity_lane: true,
            show_ghost_notes: true,
            show_scale_highlight: false,
            drum_mode: false,
            tool: PianoRollTool::Select,
            cc_drag_before: None,
        }
    }

    fn load(&mut self, clip: &MidiClip) {
        self.notes = midi_clip_content_notes(clip);
        self.selected.clear();
        self.drag = None;
        self.marquee = None;
        self.marquee_additive = false;
        self.velocity_drag_before = None;
        self.cc_drag_before = None;
        self.initialized = true;
        self.needs_fit = true;
    }
}

/// Width preset for FX Rack channel columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MixerColumnWidth {
    Narrow,
    Standard,
    Wide,
}

impl MixerColumnWidth {
    const ALL: [Self; 3] = [Self::Narrow, Self::Standard, Self::Wide];

    fn label(self) -> &'static str {
        match self {
            Self::Narrow => "Narrow",
            Self::Standard => "Standard",
            Self::Wide => "Wide",
        }
    }

    fn pixels(self) -> f32 {
        match self {
            Self::Narrow => 170.0,
            Self::Standard => 220.0,
            Self::Wide => 300.0,
        }
    }
}

/// Selected MIDI tool shown in the piano-roll side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MidiToolTab {
    Quantize,
    Transpose,
    Scale,
    Humanize,
    Legato,
    NoteLength,
    Notes,
    Transform,
    Velocity,
    Controllers,
}

impl MidiToolTab {
    const ALL: [Self; 10] = [
        Self::Quantize,
        Self::Transpose,
        Self::Scale,
        Self::Humanize,
        Self::Legato,
        Self::NoteLength,
        Self::Notes,
        Self::Transform,
        Self::Velocity,
        Self::Controllers,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Quantize => "Quantize",
            Self::Transpose => "Transpose",
            Self::Scale => "Scale",
            Self::Humanize => "Humanize",
            Self::Legato => "Legato",
            Self::NoteLength => "Note Length",
            Self::Notes => "Notes",
            Self::Transform => "Transform",
            Self::Velocity => "Velocity",
            Self::Controllers => "CC Lanes",
        }
    }
}

/// Diatonic and common pentatonic/blues scales for the conform tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaleType {
    Major,
    NaturalMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    HarmonicMinor,
    PentatonicMajor,
    PentatonicMinor,
    Blues,
}

impl ScaleType {
    const ALL: [Self; 11] = [
        Self::Major,
        Self::NaturalMinor,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::Locrian,
        Self::HarmonicMinor,
        Self::PentatonicMajor,
        Self::PentatonicMinor,
        Self::Blues,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::NaturalMinor => "Natural Minor",
            Self::Dorian => "Dorian",
            Self::Phrygian => "Phrygian",
            Self::Lydian => "Lydian",
            Self::Mixolydian => "Mixolydian",
            Self::Locrian => "Locrian",
            Self::HarmonicMinor => "Harmonic Minor",
            Self::PentatonicMajor => "Major Pentatonic",
            Self::PentatonicMinor => "Minor Pentatonic",
            Self::Blues => "Blues",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::PentatonicMajor => &[0, 2, 4, 7, 9],
            Self::PentatonicMinor => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }
}

/// Velocity shaping algorithms available in the Velocity tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VelocityMode {
    Scale,
    Compress,
    Expand,
    Curve,
    Randomize,
}

impl VelocityMode {
    const ALL: [Self; 5] = [
        Self::Scale,
        Self::Compress,
        Self::Expand,
        Self::Curve,
        Self::Randomize,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Scale => "Scale",
            Self::Compress => "Compress",
            Self::Expand => "Expand",
            Self::Curve => "Curve",
            Self::Randomize => "Randomize",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Scale => "Multiply every velocity.",
            Self::Compress => "Pull velocities toward 64.",
            Self::Expand => "Push velocities away from 64.",
            Self::Curve => "Lower or raise softer notes.",
            Self::Randomize => "Add random velocity variation.",
        }
    }
}

/// State of the floating MIDI tools window (quantize, transpose, velocity).
#[derive(Debug, Clone)]
struct MidiToolsState {
    visible_tools: BTreeSet<MidiToolTab>,
    quantize_snap: PianoRollSnap,
    quantize_strength: f32,
    quantize_swing: f32,
    transpose: i32,
    scale_root: u8,
    scale_type: ScaleType,
    velocity_mode: VelocityMode,
    velocity_factor: f32,
    velocity_amount: f32,
    velocity_curve: f32,
    velocity_random: f32,
    humanize_timing: f32,
    humanize_velocity: f32,
    legato_gap: f32,
    note_length: f32,
    join_gap: f32,
    randomize_pitch: f32,
    randomize_velocity: f32,
    randomize_timing: f32,
    randomize_density: f32,
    mirror_center: u8,
}

impl Default for MidiToolsState {
    fn default() -> Self {
        Self {
            visible_tools: BTreeSet::from([MidiToolTab::Quantize]),
            quantize_snap: PianoRollSnap::Beat,
            quantize_strength: 1.0,
            quantize_swing: 0.0,
            transpose: 0,
            scale_root: 0,
            scale_type: ScaleType::Major,
            velocity_mode: VelocityMode::Scale,
            velocity_factor: 1.0,
            velocity_amount: 50.0,
            velocity_curve: 0.0,
            velocity_random: 0.0,
            humanize_timing: 0.0,
            humanize_velocity: 0.0,
            legato_gap: 0.0,
            note_length: 480.0,
            join_gap: 0.0,
            randomize_pitch: 0.0,
            randomize_velocity: 0.0,
            randomize_timing: 0.0,
            randomize_density: 0.0,
            mirror_center: 60,
        }
    }
}

/// File-menu actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Template {
    Empty,
    Band,
    Electronic,
    Scoring,
}

impl Template {
    fn label(self) -> &'static str {
        match self {
            Template::Empty => "Empty",
            Template::Band => "Band",
            Template::Electronic => "Electronic",
            Template::Scoring => "Scoring",
        }
    }
}

/// Build a fresh project with a small set of named tracks for the chosen
/// starter template.
fn project_from_template(template: Template) -> Project {
    let mut project = Project::new("Untitled");
    project.tracks.clear();
    project.track_order.clear();

    let names: &[&str] = match template {
        Template::Empty => &["Audio 1"],
        Template::Band => &["Drums", "Bass", "Guitar", "Keys", "Vocals"],
        Template::Electronic => &["Kick", "Bass", "Synth", "FX"],
        Template::Scoring => &["Strings", "Brass", "Woodwinds", "Percussion"],
    };
    for (index, name) in names.iter().enumerate() {
        let mut track = daw_core::track::Track::new_audio(*name);
        track.color = Color::track_color(index);
        project.add_track(track);
    }
    project
}

#[derive(Debug, Clone)]
enum FileAction {
    New,
    NewTemplate(Template),
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    ClearRecent,
}

/// Metadata for one playback clip. The loader thread turns these into
/// `PlayableClip`s off the UI thread.
#[derive(Clone)]
struct PlaybackClipSpec {
    path: PathBuf,
    source_offset: u64,
    length: u64,
    start_frame: u64,
    track: u32,
    gain: f32,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
}

/// Read and trim the sources described by `specs`. Safe to call from a worker
/// thread; used for asynchronous playback refreshes.
fn prepare_playable_clips(specs: Vec<PlaybackClipSpec>) -> Vec<PlayableClip> {
    let mut playable = Vec::with_capacity(specs.len());
    for spec in specs {
        match load_wav_interleaved(&spec.path) {
            Ok((samples, channels, sample_rate)) => {
                let channel_count = channels.max(1) as usize;
                let start = (spec.source_offset as usize)
                    .saturating_mul(channel_count)
                    .min(samples.len());
                let end = start
                    .saturating_add((spec.length as usize).saturating_mul(channel_count))
                    .min(samples.len());
                if end <= start {
                    continue;
                }
                playable.push(PlayableClip {
                    samples: samples[start..end].to_vec(),
                    channels,
                    sample_rate,
                    start_frame: spec.start_frame,
                    track: spec.track,
                    gain: spec.gain,
                    fade_in: spec.fade_in,
                    fade_out: spec.fade_out,
                });
            }
            Err(e) => eprintln!("Playback: skipping {}: {}", spec.path.display(), e),
        }
    }
    playable
}

#[derive(Clone)]
struct ClipDragMember {
    clip_id: ClipId,
    from_shape: ClipShape,
    from_track: TrackId,
    last_track: TrackId,
}

/// In-progress clip drag (move, trim or fade handle).
struct ClipDrag {
    clip_id: ClipId,
    mode: DragMode,
    from_shape: ClipShape,
    from_track: TrackId,
    last_track: TrackId,
    grab_offset_secs: f64,
    /// Other selected clips moved together when `mode` is `Move`.
    members: Vec<ClipDragMember>,
    /// Selected MIDI clips moved together with the audio group.
    midi_members: Vec<MidiMoveMember>,
}

/// One MIDI clip moved as part of a multi-selection drag.
#[derive(Clone)]
struct MidiMoveMember {
    clip_id: ClipId,
    from_track: TrackId,
    from_pos: TimelinePos,
    last_track: TrackId,
}

/// In-progress MIDI clip trim or move.
#[derive(Clone)]
struct MidiClipDrag {
    clip_id: ClipId,
    mode: DragMode,
    before: MidiClip,
    from_track: TrackId,
    last_track: TrackId,
    /// Offset from the clip start to where the pointer grabbed it, in ticks.
    grab_offset_ticks: i64,
    /// Other selected MIDI clips moved together when `mode` is `Move`.
    members: Vec<MidiMoveMember>,
    /// Selected audio clips moved together with the MIDI group.
    audio_members: Vec<ClipDragMember>,
}

/// Decoded mono windows used for sample-accurate rendering at high zoom.
#[derive(Default)]
struct DetailCache {
    entries: HashMap<(PathBuf, u64, usize), Vec<f32>>,
    order: VecDeque<(PathBuf, u64, usize)>,
}

impl DetailCache {
    fn get(
        &mut self,
        path: &Path,
        start_frame: u64,
        frames: usize,
        channels: u16,
        sample_rate: u32,
    ) -> Option<&[f32]> {
        let key = (path.to_path_buf(), start_frame, frames);
        if !self.entries.contains_key(&key) {
            let window = read_mono_window(path, start_frame, frames, channels, sample_rate).ok()?;
            while self.order.len() >= DETAIL_CACHE_ENTRIES {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.order.push_back(key.clone());
            self.entries.insert(key.clone(), window);
        }
        self.entries.get(&key).map(|v| v.as_slice())
    }
}

struct AppUI {
    project: Project,
    engine: AudioEngine,
    playing: bool,
    recording: bool,
    /// Playhead position in seconds.
    playhead_secs: f64,
    /// Transport loop: when enabled, playback repeats between these points.
    loop_enabled: bool,
    loop_start_secs: f64,
    loop_end_secs: f64,
    /// Anchor while dragging a new loop range in the ruler's loop lane.
    loop_drag_anchor_secs: Option<f64>,
    /// Punch recording: capture only between these points.
    punch_enabled: bool,
    punch_in_secs: f64,
    punch_out_secs: f64,
    /// Metronome click during playback.
    metronome_enabled: bool,
    /// Recent tap-tempo intervals (seconds) and the last tap time.
    tap_intervals: VecDeque<f64>,
    last_tap: Option<std::time::Instant>,
    /// Tempo before a BPM drag, so one drag is one undo entry.
    bpm_edit_from: Option<f32>,
    /// When the last paste shortcut was handled, used to avoid double-pasting
    /// when a backend reports both a clipboard event and a key release.
    last_paste_signal: Option<std::time::Instant>,
    /// Last monitoring state pushed to the engine, so we only touch the audio
    /// streams when the requested state actually changes.
    monitoring_on: bool,
    /// Where recording started, so the clip lands at the right spot.
    record_start_secs: Option<f64>,
    /// Armed take waiting for the transport to roll (see `begin_recording`).
    pending_record_track: Option<TrackId>,
    last_playhead_update: std::time::Instant,
    dragging_playhead: bool,
    input_devices: Vec<daw_engine::audio_backend::AudioDeviceInfo>,
    selected_input_device: Option<String>,
    midi_inputs: Vec<daw_engine::MidiInputInfo>,
    selected_midi_input: Option<String>,
    /// Optional MIDI channel filter (0-15). `None` means all channels.
    midi_input_channel: Option<u8>,
    midi_monitor: Option<daw_engine::MidiMonitor>,
    midi_activity: daw_engine::MidiActivity,
    /// Available MIDI output ports for MIDI thru.
    midi_outputs: Vec<daw_engine::MidiOutputInfo>,
    selected_midi_output: Option<String>,
    midi_thru: Option<daw_engine::MidiThru>,
    midi_thru_enabled: bool,
    /// Whether live MIDI notes are currently being auditioned through the
    /// output stream's sine synth.
    midi_audition_on: bool,
    /// Quantize recorded MIDI notes to the Arrange snap grid.
    input_quantize: bool,
    error_message: Option<String>,
    /// Yellow banner shown when free disk space is low during recording.
    disk_warning: Option<String>,
    last_disk_check: std::time::Instant,

    // Timeline view state
    view_start_secs: f64,
    pixels_per_sec: f32,
    needs_fit: bool,
    last_timeline_width: f32,
    selected_clip: Option<ClipId>,
    /// All selected clips. `selected_clip` is the primary/last-clicked one.
    selected_clips: BTreeSet<ClipId>,
    /// Selected MIDI clip (mutually exclusive with the audio selection).
    selected_midi_clip: Option<ClipId>,
    /// All selected MIDI clips. `selected_midi_clip` is the primary one.
    selected_midi_clips: BTreeSet<ClipId>,

    // Non-destructive editing
    snap_mode: SnapMode,
    /// How the ruler labels positions (musical bars/beats or clock time).
    ruler_mode: RulerMode,
    /// Snap clip moves relative to the clip's original position, preserving an
    /// off-grid offset instead of pulling the clip onto the absolute grid.
    snap_relative: bool,
    /// Alt bypasses snapping while held.
    snap_suspended: bool,
    drag: Option<ClipDrag>,
    /// In-progress MIDI clip trim drag.
    midi_drag: Option<MidiClipDrag>,
    /// Track currently being dragged in the track list.
    track_drag: Option<TrackId>,
    /// Rubber-band selection while dragging empty track space.
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    marquee_additive: bool,
    clipboard: Vec<AudioClip>,
    /// MIDI clips copied from the arrange view.
    midi_clipboard: Vec<MidiClip>,
    /// Floating clip-audio editor, opened from the Arrange **Audio…** button.
    audio_window: Option<ClipAudioWindow>,
    /// Logic-style insert rack showing every track as a horizontal column.
    fx_rack_open: bool,
    /// Column width preset used by the FX Rack.
    fx_rack_column_width: MixerColumnWidth,
    /// Smoothed peak levels shown in the FX Rack, keyed by track.
    track_meter_levels: HashMap<TrackId, f32>,
    /// Peak-hold ticks for the FX Rack track meters.
    track_meter_holds: HashMap<TrackId, f32>,
    /// Smoothed master L/R levels shown in the FX Rack header.
    master_meter_levels: [f32; 2],
    /// Insert chain copied from an FX Rack column.
    insert_clipboard: Option<Vec<InsertSlot>>,
    /// Original insert chain for the current effect-parameter gesture, pushed
    /// as one undo entry when the pointer is released.
    pending_insert_edit: Option<(TrackId, Vec<InsertSlot>)>,
    /// Floating piano-roll view for the selected MIDI clip.
    piano_roll: Option<PianoRollWindow>,
    /// Pitch currently auditioned from the piano roll, with its start time so
    /// clicks produce an audible note instead of a one-frame blip.
    piano_roll_preview: Option<(u8, std::time::Instant)>,
    /// Where a trashed clip was last attached, so Restore can put it back.
    deleted_from: HashMap<ClipId, TrackId>,
    history: History<EditAction>,
    /// While set, `push_undo` collects actions into one labelled history
    /// entry instead of pushing each one separately.
    undo_group: Option<(String, Vec<EditAction>)>,
    /// Pending multi-clip gain/fade/colour edit: label + per-clip shape before
    /// the gesture. Committed on pointer release as one undo step.
    pending_shape_edit: Option<(String, Vec<(ClipId, ClipShape)>)>,
    /// Mute actions applied when a take starts, rolled into the take's undo
    /// group when recording stops.
    pending_take_mutes: Vec<EditAction>,
    /// MIDI take state.
    recording_midi: bool,
    midi_record_track: Option<TrackId>,
    midi_captured: Vec<daw_engine::MidiInputEvent>,
    /// Local time when the MIDI take started, so notes keep their offset from
    /// the playhead even if the first note arrives late.
    midi_record_started_at: Option<std::time::Instant>,
    /// Recently opened/saved projects (most recent first).
    recent_projects: Vec<PathBuf>,
    /// Newer autosave found when opening a project.
    recovery_offer: Option<PathBuf>,
    last_autosave: std::time::Instant,
    autosave_status: Option<String>,
    /// True while consecutive edits (e.g. a slider drag) belong to one gesture
    /// and should share a single undo entry.
    merge_chain: bool,
    /// Whether the floating History panel is visible.
    history_open: bool,
    /// Deferred mixer refresh for edits made while a pointer gesture is active.
    needs_audio_refresh: bool,

    // Waveform overview generation
    waveform_tx: mpsc::Sender<(ClipId, PathBuf, Result<WaveformOverview>)>,
    waveform_rx: mpsc::Receiver<(ClipId, PathBuf, Result<WaveformOverview>)>,
    detail_cache: DetailCache,
    // Asynchronous playback reloads (WAV loading happens off the UI thread)
    playback_spec_tx: mpsc::Sender<(u64, Vec<PlaybackClipSpec>)>,
    playback_ready_rx: mpsc::Receiver<(u64, Vec<PlayableClip>)>,
    playback_generation: u64,
}

impl AppUI {
    fn new(project: Project, engine: AudioEngine) -> Self {
        let input_devices = engine.input_devices().unwrap_or_default();
        let midi_inputs = daw_engine::list_midi_inputs().unwrap_or_default();
        let midi_outputs = daw_engine::list_midi_outputs().unwrap_or_default();
        let (waveform_tx, waveform_rx) = mpsc::channel();

        // Playback reloads read from disk; keep that off the UI thread.
        let (playback_spec_tx, playback_spec_rx) =
            mpsc::channel::<(u64, Vec<PlaybackClipSpec>)>();
        let (playback_ready_tx, playback_ready_rx) =
            mpsc::channel::<(u64, Vec<PlayableClip>)>();
        std::thread::spawn(move || {
            while let Ok((generation, specs)) = playback_spec_rx.recv() {
                let clips = prepare_playable_clips(specs);
                if playback_ready_tx.send((generation, clips)).is_err() {
                    break;
                }
            }
        });

        let mut app = Self {
            project,
            engine,
            playing: false,
            recording: false,
            playhead_secs: 0.0,
            loop_enabled: false,
            loop_start_secs: 0.0,
            loop_end_secs: 0.0,
            loop_drag_anchor_secs: None,
            punch_enabled: false,
            punch_in_secs: 0.0,
            punch_out_secs: 0.0,
            metronome_enabled: false,
            tap_intervals: VecDeque::new(),
            last_tap: None,
            bpm_edit_from: None,
            last_paste_signal: None,
            monitoring_on: false,
            record_start_secs: None,
            pending_record_track: None,
            last_playhead_update: std::time::Instant::now(),
            dragging_playhead: false,
            input_devices,
            selected_input_device: None,
            midi_inputs,
            selected_midi_input: None,
            midi_input_channel: None,
            midi_monitor: None,
            midi_activity: daw_engine::MidiActivity::default(),
            midi_outputs,
            selected_midi_output: None,
            midi_thru: None,
            midi_thru_enabled: false,
            midi_audition_on: false,
            input_quantize: false,
            error_message: None,
            disk_warning: None,
            last_disk_check: std::time::Instant::now(),
            view_start_secs: 0.0,
            pixels_per_sec: 100.0,
            needs_fit: true,
            last_timeline_width: 1000.0,
            selected_clip: None,
            selected_clips: BTreeSet::new(),
            selected_midi_clip: None,
            selected_midi_clips: BTreeSet::new(),
            snap_mode: SnapMode::Beat,
            snap_relative: false,
            ruler_mode: RulerMode::BarsBeats,
            snap_suspended: false,
            drag: None,
            midi_drag: None,
            track_drag: None,
            marquee: None,
            marquee_additive: false,
            clipboard: Vec::new(),
            midi_clipboard: Vec::new(),
            audio_window: None,
            fx_rack_open: false,
            fx_rack_column_width: MixerColumnWidth::Standard,
            track_meter_levels: HashMap::new(),
            track_meter_holds: HashMap::new(),
            master_meter_levels: [0.0, 0.0],
            insert_clipboard: None,
            pending_insert_edit: None,
            piano_roll: None,
            piano_roll_preview: None,
            deleted_from: HashMap::new(),
            history: History::new(1000),
            undo_group: None,
            pending_shape_edit: None,
            pending_take_mutes: Vec::new(),
            recording_midi: false,
            midi_record_track: None,
            midi_captured: Vec::new(),
            midi_record_started_at: None,
            recent_projects: Self::load_recent_projects(),
            recovery_offer: None,
            last_autosave: std::time::Instant::now(),
            autosave_status: None,
            merge_chain: false,
            history_open: false,
            needs_audio_refresh: false,
            waveform_tx,
            waveform_rx,
            detail_cache: DetailCache::default(),
            playback_spec_tx,
            playback_ready_rx,
            playback_generation: 0,
        };
        // Projects loaded from disk may predate stored overviews; generate any
        // that are missing in the background.
        app.spawn_missing_overviews();
        app.sync_monitoring();
        // Auto-connect the first real MIDI keyboard, skipping ALSA's virtual
        // "Midi Through" port.
        if let Some(device) = app
            .midi_inputs
            .iter()
            .find(|device| !device.name.contains("Midi Through"))
            .cloned()
        {
            app.selected_midi_input = Some(device.name);
            app.restart_midi_monitor();
        }
        app
    }

    fn timeline_duration(&self) -> f64 {
        self.project.duration_samples() as f64 / self.project.sample_rate.max(1) as f64
    }

    fn spawn_missing_overviews(&mut self) {
        let missing: Vec<(ClipId, PathBuf)> = self
            .project
            .audio_clips
            .iter()
            .filter(|(_, clip)| clip.source.overview.is_none() && clip.source.path.exists())
            .map(|(id, clip)| (id, clip.source.path.clone()))
            .collect();

        for (clip_id, path) in missing {
            self.spawn_overview(clip_id, path);
        }
    }

    fn spawn_overview(&self, clip_id: ClipId, path: PathBuf) {
        let tx = self.waveform_tx.clone();
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let result = WaveformOverview::generate_from_file(&thread_path, FRAMES_PER_PIXEL);
            let _ = tx.send((clip_id, thread_path, result));
        });
    }

    fn poll_waveforms(&mut self) {
        loop {
            match self.waveform_rx.try_recv() {
                Ok((clip_id, path, Ok(overview))) => {
                    if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
                        // Ignore stale results if the clip now points at a
                        // different file (e.g. after reversing it).
                        if clip.source.path == path {
                            clip.source.overview = Some(overview);
                        }
                    }
                }
                Ok((_, _, Err(e))) => eprintln!("Waveform generation failed: {}", e),
                Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
    }

    fn fit_view(&mut self, width: f32) {
        let duration = self.timeline_duration();
        let span = (duration * 1.05).max(MIN_TIMELINE_SECS);
        self.view_start_secs = 0.0;
        self.pixels_per_sec = (width as f64 / span) as f32;
        self.pixels_per_sec = self.pixels_per_sec.clamp(MIN_PPS, MAX_PPS);
        self.needs_fit = false;
    }

    fn clamp_view(&mut self, width: f32) {
        let view_len = width as f64 / self.pixels_per_sec.max(MIN_PPS) as f64;
        // The timeline is never shorter than the project content or the
        // playhead, so the playhead can sit past the last clip, and half a view
        // of empty space is always available beyond the end for placing new
        // material.
        let content_end = self
            .timeline_duration()
            .max(self.playhead_secs)
            .max(MIN_TIMELINE_SECS);
        let max_start = (content_end - view_len).max(0.0) + view_len * 0.5;
        self.view_start_secs = self.view_start_secs.clamp(0.0, max_start);
    }

    fn zoom_at(&mut self, rect: egui::Rect, pointer_x: f32, factor: f32) {
        let view = TimelineView {
            start_secs: self.view_start_secs,
            pps: self.pixels_per_sec,
        };
        let anchor_t = view.t(rect, pointer_x);
        let new_pps = (self.pixels_per_sec * factor).clamp(MIN_PPS, MAX_PPS);
        self.pixels_per_sec = new_pps;
        self.view_start_secs = anchor_t - ((pointer_x - rect.left()) / new_pps) as f64;
        self.clamp_view(rect.width());
    }

    fn zoom_center(&mut self, factor: f32) {
        let width = self.last_timeline_width.max(1.0);
        let view_len = width as f64 / self.pixels_per_sec.max(MIN_PPS) as f64;
        let center_t = self.view_start_secs + view_len * 0.5;
        self.pixels_per_sec = (self.pixels_per_sec * factor).clamp(MIN_PPS, MAX_PPS);
        let new_len = width as f64 / self.pixels_per_sec.max(MIN_PPS) as f64;
        self.view_start_secs = center_t - new_len * 0.5;
        self.clamp_view(width);
    }
}

impl AppUI {
    // ---- Non-destructive editing -------------------------------------------------

    fn time_signature(&self) -> (u8, u8) {
        self.project.time_signature_map.time_sig_at_tick(0)
    }

    /// Length of one signature beat (the denominator note) in seconds.
    fn secs_per_beat(&self) -> f64 {
        let bpm = self.project.tempo_map.default_bpm().max(1.0) as f64;
        let (_, denominator) = self.time_signature();
        60.0 / bpm * (4.0 / denominator.max(1) as f64)
    }

    /// Current snap spacing in seconds, or `None` when snapping is off/Alt is
    /// held.
    fn snap_grid_secs(&self) -> Option<f64> {
        if self.snap_suspended {
            return None;
        }
        let (numerator, _) = self.time_signature();
        self.snap_mode
            .interval_secs(self.secs_per_beat(), numerator.max(1) as f64)
            .filter(|grid| *grid > 1e-6)
    }

    /// Snap `t` to the current grid (Alt bypasses snapping).
    fn snap_secs(&self, t: f64) -> f64 {
        let t = t.max(0.0);
        match self.snap_grid_secs() {
            Some(grid) => (t / grid).round() * grid,
            None => t,
        }
    }

    /// Build the ruler/grid tick lines for the visible range in the current
    /// ruler mode. In Bars/Beats mode this labels every beat when there is
    /// room, otherwise every 2/4/8... bars.
    fn build_ruler_ticks(&self, start_secs: f64, end_secs: f64) -> Vec<RulerTick> {
        let mut ticks = Vec::new();
        let start = start_secs.max(0.0);
        if end_secs <= start {
            return ticks;
        }

        match self.ruler_mode {
            RulerMode::Time => {
                let major = nice_interval(self.pixels_per_sec);
                let minor = major / 4.0;

                let mut t = (start / minor).floor() * minor;
                let mut guard = 0;
                while t <= end_secs && guard < 4096 {
                    if t >= 0.0 {
                        ticks.push(RulerTick { secs: t, label: None, major: false });
                    }
                    t += minor;
                    guard += 1;
                }

                let mut t = (start / major).floor() * major;
                let mut guard = 0;
                while t <= end_secs && guard < 4096 {
                    if t >= 0.0 {
                        ticks.push(RulerTick {
                            secs: t,
                            label: Some(format_ruler_time(t, major)),
                            major: true,
                        });
                    }
                    t += major;
                    guard += 1;
                }
            }
            RulerMode::BarsBeats => {
                let (numerator, denominator) = self.time_signature();
                let bpm = self.project.tempo_map.default_bpm().max(1.0) as f64;
                let secs_per_tick = 60.0 / bpm / 960.0;
                let beat_ticks = 960.0 * 4.0 / denominator.max(1) as f64;
                let bar_ticks = beat_ticks * numerator.max(1) as f64;
                let beat_secs = beat_ticks * secs_per_tick;
                let bar_secs = bar_ticks * secs_per_tick;
                let beat_px = beat_secs * self.pixels_per_sec as f64;
                let bar_px = bar_secs * self.pixels_per_sec as f64;

                // Label each beat when there is room; otherwise space the bar
                // labels out in powers of two.
                let label_beats = beat_px >= 44.0;
                let major_step = if label_beats {
                    beat_ticks
                } else {
                    let mut bars_per_label = 1.0;
                    while bar_px * bars_per_label < 64.0 && bars_per_label < 4096.0 {
                        bars_per_label *= 2.0;
                    }
                    bar_ticks * bars_per_label
                };
                let minor_step = if label_beats {
                    beat_ticks / 4.0
                } else if beat_px >= 6.0 {
                    beat_ticks
                } else {
                    bar_ticks
                };

                let start_tick = start / secs_per_tick;
                let end_tick = end_secs / secs_per_tick;

                let mut tick = (start_tick / minor_step).floor() * minor_step;
                let mut guard = 0;
                while tick <= end_tick && guard < 4096 {
                    if tick >= 0.0 {
                        ticks.push(RulerTick {
                            secs: tick * secs_per_tick,
                            label: None,
                            major: false,
                        });
                    }
                    tick += minor_step;
                    guard += 1;
                }

                let mut tick = (start_tick / major_step).floor() * major_step;
                let mut guard = 0;
                while tick <= end_tick && guard < 4096 {
                    if tick >= 0.0 {
                        let bar_index = (tick / bar_ticks).floor().max(0.0) as u64 + 1;
                        let label = if label_beats {
                            // Tiny epsilon so float drift at bar boundaries
                            // still reports beat 1, not the last beat.
                            let beat_index = ((tick.rem_euclid(bar_ticks) / beat_ticks)
                                + 1e-6)
                                .floor()
                                .max(0.0) as u64
                                + 1;
                            format!("{}.{}", bar_index, beat_index)
                        } else {
                            format!("{}", bar_index)
                        };
                        ticks.push(RulerTick {
                            secs: tick * secs_per_tick,
                            label: Some(label),
                            major: true,
                        });
                    }
                    tick += major_step;
                    guard += 1;
                }
            }
        }

        ticks
    }

    /// Move `clip_id` onto `track_id`, removing it from any other track.
    fn place_clip(&mut self, clip_id: ClipId, track_id: TrackId) {
        for track in self.project.tracks.values_mut() {
            track.remove_clip(clip_id);
        }
        self.attach_clip(track_id, clip_id);
    }

    fn attach_clip(&mut self, track_id: TrackId, clip_id: ClipId) {
        if let Some(track) = self.project.tracks.get_mut(&track_id) {
            if !track.clips.contains(&clip_id) {
                track.clips.push(clip_id);
            }
            self.deleted_from.remove(&clip_id);
        }
    }

    fn detach_clip(&mut self, track_id: TrackId, clip_id: ClipId) {
        if let Some(track) = self.project.tracks.get_mut(&track_id) {
            track.remove_clip(clip_id);
        }
        if self.project.audio_clips.contains_key(clip_id) {
            self.deleted_from.insert(clip_id, track_id);
        }
    }

    fn clip_track(&self, clip_id: ClipId) -> Option<TrackId> {
        self.project
            .track_order
            .iter()
            .find(|track_id| {
                self.project
                    .tracks
                    .get(*track_id)
                    .map(|track| track.clips.contains(&clip_id))
                    .unwrap_or(false)
            })
            .copied()
    }

    fn midi_clip_track(&self, clip_id: ClipId) -> Option<TrackId> {
        self.project
            .track_order
            .iter()
            .find(|track_id| {
                self.project
                    .tracks
                    .get(*track_id)
                    .map(|track| track.midi_clips.contains(&clip_id))
                    .unwrap_or(false)
            })
            .copied()
    }

    fn place_midi_clip(&mut self, clip_id: ClipId, track_id: TrackId) {
        for track in self.project.tracks.values_mut() {
            if track.id != track_id {
                track.remove_midi_clip(clip_id);
            }
        }
        if let Some(track) = self.project.tracks.get_mut(&track_id) {
            track.add_midi_clip(clip_id);
        }
    }

    /// Find the contiguous run of clips on the selected clip's track. A run
    /// grows while clips touch (`end >= next.start`) or overlap.
    fn consolidation_plan(&self, clip_id: ClipId) -> Option<ConsolidationPlan> {
        let track_id = self.clip_track(clip_id)?;
        let track = self.project.tracks.get(&track_id)?;
        let project_sr = self.project.sample_rate.max(1) as f64;

        let mut spans: Vec<(ClipId, u64, u64)> = track
            .clips
            .iter()
            .filter_map(|id| {
                let clip = self.project.audio_clips.get(*id)?;
                if clip.length == 0 {
                    return None;
                }
                let start = clip.timeline_position.sample;
                let source_sr = clip.source.sample_rate.max(1) as f64;
                let out_frames =
                    (clip.length as f64 * project_sr / source_sr).ceil() as u64;
                let end = start.saturating_add(out_frames);
                Some((*id, start, end))
            })
            .collect();
        spans.sort_by_key(|(_, start, end)| (*start, *end));
        let (clip_ids, start, end) = contiguous_clip_span(&spans, clip_id)?;

        Some(ConsolidationPlan {
            track: track_id,
            clip_ids,
            start,
            end,
        })
    }

    fn after_edit(&mut self) {
        self.project.modified = true;
    }

    /// Reload the mixer so playback reflects the edited arrangement.
    fn refresh_playback_if_playing(&mut self) {
        // Rebuilding the mixer reads WAVs; do it on the worker thread so the
        // UI and playhead keep moving.
        self.request_playback_reload();
    }

    /// Update only the MIDI note list on the live mixer. Used while dragging
    /// piano-roll notes so audio clips are not re-read from disk every frame.
    fn refresh_midi_playback(&mut self) {
        if !self.playing {
            return;
        }
        let out_rate = self.engine.output_sample_rate().max(1);
        let notes = self.collect_playable_notes(out_rate);
        self.engine.set_playback_notes(notes);
    }

    /// Write the piano-roll note buffer into the project clip.
    fn write_piano_roll_to_project(&mut self, window: &PianoRollWindow) {
        let Some(clip) = self.project.midi_clips.get_mut(window.clip_id) else {
            return;
        };
        write_piano_roll_notes(clip, &window.notes);
        self.after_edit();
        self.refresh_midi_playback();
    }

    /// Push one undo entry for a completed piano-roll edit.
    fn push_midi_clip_undo(&mut self, clip_id: ClipId, before: MidiClip) {
        let Some(after) = self.project.midi_clips.get(clip_id).cloned() else {
            return;
        };
        if before != after {
            self.push_undo(EditAction::MidiClip { clip_id, from: before, to: after }, false);
        }
    }

    /// Apply a MIDI note transform to every selected MIDI clip as one undo step.
    /// Apply a MIDI note transform to a specific set of clips.
    fn apply_midi_note_edit_to_clips(
        &mut self,
        ids: Vec<ClipId>,
        label: &str,
        edit: impl Fn(&mut Vec<PianoRollNote>),
    ) {
        if ids.is_empty() {
            return;
        }
        let mut actions = Vec::new();
        for clip_id in ids {
            let Some(clip) = self.project.midi_clips.get(clip_id).cloned() else {
                continue;
            };
            let mut notes = midi_clip_content_notes(&clip);
            let before = notes.clone();
            edit(&mut notes);
            if notes == before {
                continue;
            }
            let mut to = clip.clone();
            write_piano_roll_notes(&mut to, &notes);
            if to != clip {
                actions.push(EditAction::MidiClip {
                    clip_id,
                    from: clip,
                    to,
                });
            }
        }
        if actions.is_empty() {
            return;
        }
        let action = if actions.len() == 1 {
            actions.pop().unwrap()
        } else {
            EditAction::Group {
                actions,
                label: label.to_string(),
            }
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
        self.refresh_playback_if_playing();
        self.reload_piano_roll();
    }

    /// Force the open piano roll to reload from the project after undo/redo.
    fn reload_piano_roll(&mut self) {
        if let Some(window) = &mut self.piano_roll {
            window.initialized = false;
            window.selected.clear();
            window.drag = None;
        }
        self.stop_piano_roll_preview();
    }

    /// Audition a note from the piano roll through the live synth. Clicks get a
    /// short note; moving to a new pitch retriggers it.
    fn preview_piano_roll_note(&mut self, pitch: u8, velocity: u8) {
        self.stop_piano_roll_preview();
        if !self.engine.is_running() {
            let _ = self.engine.start();
        }
        self.engine.note_on(pitch, velocity.max(1));
        self.piano_roll_preview = Some((pitch, std::time::Instant::now()));
    }

    fn stop_piano_roll_preview(&mut self) {
        if let Some((pitch, _)) = self.piano_roll_preview.take() {
            self.engine.note_off(pitch);
        }
    }

    /// Stop a piano-roll preview after a short audible tail.
    fn poll_piano_roll_preview(&mut self) {
        if let Some((pitch, started)) = self.piano_roll_preview {
            if started.elapsed() >= std::time::Duration::from_millis(300) {
                self.engine.note_off(pitch);
                self.piano_roll_preview = None;
            }
        }
    }

    /// Enable live input monitoring when any track asks for it: `On` always,
    /// `Auto` while the track is armed or a take is running.
    fn sync_monitoring(&mut self) {
        let enabled = self.project.tracks.values().any(|track| {
            track.track_type == TrackType::Audio
                && match track.monitor_mode {
                    MonitorMode::Off => false,
                    MonitorMode::Auto => track.record_arm || self.recording,
                    MonitorMode::On => true,
                }
        });
        if enabled != self.monitoring_on {
            self.monitoring_on = enabled;
            self.engine.set_monitoring(enabled);
        }
    }

    /// Whether live MIDI input should be auditioned through the built-in sine
    /// synth. MIDI and instrument tracks follow the same Off/Auto/On monitor
    /// switch as audio tracks, so an armed track is heard while recording.
    fn midi_audition_enabled(&self) -> bool {
        self.project.tracks.values().any(|track| {
            matches!(track.track_type, TrackType::Midi | TrackType::Instrument)
                && match track.monitor_mode {
                    MonitorMode::Off => false,
                    MonitorMode::Auto => track.record_arm || self.recording_midi,
                    MonitorMode::On => true,
                }
        })
    }

    /// Stop all hanging MIDI notes and clear scheduled playback notes.
    fn panic_midi(&mut self) {
        self.engine.all_notes_off();
        self.engine.set_playback_notes(Vec::new());
        self.piano_roll_preview = None;
        self.midi_activity = daw_engine::MidiActivity::default();
        if let Some(thru) = self.midi_thru.as_mut() {
            let _ = thru.all_notes_off();
        }
    }

    /// Track whose fader/pan should shape live MIDI audition. Prefer the armed
    /// track, then the track containing the open piano-roll clip, then the
    /// first monitored MIDI/instrument track.
    fn live_midi_track(&self) -> Option<&Track> {
        if let Some(track) = self.project.track_order.iter().find_map(|id| {
            self.project.tracks.get(id).filter(|track| {
                matches!(track.track_type, TrackType::Midi | TrackType::Instrument)
                    && track.record_arm
            })
        }) {
            return Some(track);
        }
        if let Some(window) = &self.piano_roll {
            if let Some(track) = self.project.track_order.iter().find_map(|id| {
                self.project.tracks.get(id).filter(|track| {
                    track.midi_clips.contains(&window.clip_id)
                })
            }) {
                return Some(track);
            }
        }
        self.project.track_order.iter().find_map(|id| {
            self.project.tracks.get(id).filter(|track| {
                matches!(track.track_type, TrackType::Midi | TrackType::Instrument)
                    && match track.monitor_mode {
                        MonitorMode::Off => false,
                        MonitorMode::Auto => {
                            track.record_arm || self.recording_midi
                        }
                        MonitorMode::On => true,
                    }
            })
        })
    }

    /// Keep the live audition synth in step with the active track's fader/pan.
    fn sync_live_midi_gains(&mut self) {
        let any_solo = self.project.tracks.values().any(|track| track.solo);
        let gains = self
            .live_midi_track()
            .map(|track| {
                let gain = audible_track_gain(track, any_solo).unwrap_or(0.0);
                (
                    track.pan.left_gain * gain,
                    track.pan.right_gain * gain,
                )
            })
            .unwrap_or((1.0, 1.0));
        self.engine.set_live_midi_gains(gains.0, gains.1);
    }

    fn clear_selection(&mut self) {
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_midi_clip = None;
        self.selected_midi_clips.clear();
    }

    fn set_single_selection(&mut self, clip_id: ClipId) {
        self.selected_clips.clear();
        self.selected_clips.insert(clip_id);
        self.selected_clip = Some(clip_id);
        self.selected_midi_clip = None;
        self.selected_midi_clips.clear();
    }

    fn add_to_selection(&mut self, clip_id: ClipId) {
        self.selected_clips.insert(clip_id);
        self.selected_clip = Some(clip_id);
    }

    /// Ctrl/Cmd-click: add the clip, or remove it if it was already selected.
    fn toggle_selection(&mut self, clip_id: ClipId) {
        if self.selected_clips.remove(&clip_id) {
            if self.selected_clip == Some(clip_id) {
                self.selected_clip = self.selected_clips.iter().next().copied();
            }
        } else {
            self.selected_clips.insert(clip_id);
            self.selected_clip = Some(clip_id);
        }
    }

    fn selection_ids(&self) -> Vec<ClipId> {
        self.selected_clips.iter().copied().collect()
    }

    fn set_single_midi_selection(&mut self, clip_id: ClipId) {
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_midi_clips.clear();
        self.selected_midi_clips.insert(clip_id);
        self.selected_midi_clip = Some(clip_id);
    }

    fn add_to_midi_selection(&mut self, clip_id: ClipId) {
        self.selected_midi_clips.insert(clip_id);
        self.selected_midi_clip = Some(clip_id);
    }

    /// Ctrl/Cmd-click for MIDI clips.
    fn toggle_midi_selection(&mut self, clip_id: ClipId) {
        if self.selected_midi_clips.remove(&clip_id) {
            if self.selected_midi_clip == Some(clip_id) {
                self.selected_midi_clip =
                    self.selected_midi_clips.iter().next().copied();
            }
        } else {
            self.selected_midi_clips.insert(clip_id);
            self.selected_midi_clip = Some(clip_id);
        }
    }

    fn midi_selection_ids(&self) -> Vec<ClipId> {
        self.selected_midi_clips.iter().copied().collect()
    }

    fn push_undo(&mut self, action: EditAction, merge: bool) {
        if let Some((_, actions)) = &mut self.undo_group {
            actions.push(action);
            return;
        }

        // Merge continuous edits of the same clip (e.g. dragging a gain slider)
        // into a single history entry while the pointer stays down.
        if merge && self.merge_chain && merge_continuous_edit(&mut self.history, &action) {
            return;
        }

        let description = action.description();
        self.history.push(action, description);
        self.merge_chain = merge;
    }

    /// Start collecting undo entries into one labelled history step.
    fn begin_undo_group(&mut self, label: impl Into<String>) {
        self.undo_group = Some((label.into(), Vec::new()));
    }

    /// Finish the active undo group. A single action is pushed directly; several
    /// are wrapped in a labelled `Group`.
    fn end_undo_group(&mut self) {
        let Some((label, actions)) = self.undo_group.take() else {
            return;
        };
        match actions.len() {
            0 => {}
            1 => self.push_undo(actions.into_iter().next().unwrap(), false),
            _ => self.push_undo(EditAction::Group { actions, label }, false),
        }
    }

    /// Commit a multi-clip gain/fade/colour gesture as one history entry.
    fn commit_pending_shape_edit(&mut self) {
        let Some((label, before)) = self.pending_shape_edit.take() else {
            return;
        };
        let mut actions = Vec::new();
        for (clip_id, from) in before {
            let Some(clip) = self.project.audio_clips.get(clip_id) else {
                continue;
            };
            let to = ClipShape::of(clip);
            if to != from {
                actions.push(EditAction::Shape {
                    clip_id,
                    from,
                    to,
                });
            }
        }
        if actions.is_empty() {
            return;
        }
        if actions.len() == 1 {
            self.push_undo(actions.pop().unwrap(), false);
        } else {
            self.push_undo(EditAction::Group { actions, label }, false);
        }
        self.after_edit();
    }

    /// Commit a track-insert parameter gesture as one undo entry.
    fn commit_pending_insert_edit(&mut self) {
        let Some((track_id, from)) = self.pending_insert_edit.take() else {
            return;
        };
        let Some(track) = self.project.tracks.get(&track_id) else {
            return;
        };
        let to = track.inserts.clone();
        if to != from {
            self.push_undo(EditAction::TrackInserts { track_id, from, to }, false);
        }
    }

    /// Apply an edited insert chain and keep the live mixer in step. While a
    /// slider is dragged the original chain is remembered so the whole gesture
    /// becomes a single undo step.
    fn apply_track_inserts(&mut self, track_id: TrackId, inserts: Vec<InsertSlot>) {
        if self
            .pending_insert_edit
            .as_ref()
            .map_or(false, |(pending, _)| *pending != track_id)
        {
            self.commit_pending_insert_edit();
        }
        let before = self
            .pending_insert_edit
            .as_ref()
            .filter(|(pending, _)| *pending == track_id)
            .map(|(_, before)| before.clone())
            .or_else(|| {
                self.project
                    .tracks
                    .get(&track_id)
                    .map(|track| track.inserts.clone())
            })
            .unwrap_or_default();
        let Some(track) = self.project.tracks.get_mut(&track_id) else {
            return;
        };
        track.inserts = inserts;
        if self.pending_insert_edit.is_none() {
            self.pending_insert_edit = Some((track_id, before));
        }
        self.after_edit();
        let tracks = self.collect_playable_tracks();
        self.engine.set_playback_tracks(tracks);
    }

    /// Mute unmuted clips on `track_id` whose span contains `position`, so an
    /// existing take goes silent as soon as a new one starts recording over it.
    fn mute_takes_at(&mut self, track_id: TrackId, position: u64) -> Vec<EditAction> {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let Some(track) = self.project.tracks.get(&track_id) else {
            return Vec::new();
        };
        let clip_ids = track.clips.clone();
        let mut actions = Vec::new();
        for clip_id in clip_ids {
            let Some(clip) = self.project.audio_clips.get(clip_id) else {
                continue;
            };
            if clip.muted || clip.locked || clip.length == 0 {
                continue;
            }
            let (start, end) = clip_span_samples(clip, project_sr);
            if start <= position && position < end {
                let from = ClipShape::of(clip);
                let mut to = from;
                to.muted = true;
                if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
                    to.apply(clip);
                }
                actions.push(EditAction::Shape {
                    clip_id,
                    from,
                    to,
                });
            }
        }
        actions
    }

    /// Push any take-start mutes that never made it into a recording group.
    fn flush_pending_take_mutes(&mut self) {
        if self.pending_take_mutes.is_empty() {
            return;
        }
        let actions = std::mem::take(&mut self.pending_take_mutes);
        let label = if actions.len() == 1 {
            "Mute take".to_string()
        } else {
            format!("Mute {} takes", actions.len())
        };
        self.push_undo(EditAction::Group { actions, label }, false);
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Mute or unmute every selected clip as one undo step.
    fn toggle_selected_clips_muted(&mut self) {
        let midi_ids = self.midi_selection_ids();
        let audio_ids = self.selection_ids();
        if midi_ids.is_empty() && audio_ids.is_empty() {
            return;
        }

        let midi_all_muted = midi_ids.iter().all(|id| {
            self.project
                .midi_clips
                .get(*id)
                .map_or(true, |clip| clip.muted)
        });
        let audio_all_muted = audio_ids.iter().all(|id| {
            self.project
                .audio_clips
                .get(*id)
                .map_or(true, |clip| clip.muted)
        });
        let muted = !(midi_all_muted && audio_all_muted);
        self.begin_undo_group(if muted { "Mute clips" } else { "Unmute clips" });
        for id in midi_ids {
            let Some(clip) = self.project.midi_clips.get(id).cloned() else {
                continue;
            };
            if clip.muted == muted {
                continue;
            }
            let mut to = clip.clone();
            to.muted = muted;
            let action = EditAction::MidiClip {
                clip_id: id,
                from: clip,
                to,
            };
            self.apply_action(&action, true);
            self.push_undo(action, false);
        }
        for id in audio_ids {
            self.set_clip_muted(id, muted);
        }
        self.end_undo_group();
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Set a clip's mute flag as an undoable Shape action.
    fn set_clip_muted(&mut self, clip_id: ClipId, muted: bool) {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        let from = ClipShape::of(clip);
        if from.muted == muted {
            return;
        }
        let mut to = from;
        to.muted = muted;
        let action = EditAction::Shape {
            clip_id,
            from,
            to,
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
    }

    /// After a take, keep only the overlapping part of an old take muted:
    /// split the old clip around the new take's span and leave the outer
    /// pieces audible.
    fn carve_old_take_for_span(&mut self, clip_id: ClipId, new_start: u64, new_end: u64) {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked || clip.length == 0 {
            return;
        }
        let (clip_start, clip_end) = clip_span_samples(clip, project_sr);
        let overlap_start = clip_start.max(new_start);
        let overlap_end = clip_end.min(new_end);
        if overlap_start >= overlap_end {
            return;
        }

        if overlap_start <= clip_start && overlap_end >= clip_end {
            // The whole old take is covered; it stays muted.
            self.set_clip_muted(clip_id, true);
        } else if overlap_start <= clip_start {
            // Covered at the start: unmute the tail after the take.
            if let Some(right) = self.split_clip_at_sample(clip_id, overlap_end) {
                self.set_clip_muted(right, false);
            }
            self.set_clip_muted(clip_id, true);
        } else if overlap_end >= clip_end {
            // Covered at the end: unmute the portion before the take.
            if let Some(right) = self.split_clip_at_sample(clip_id, overlap_start) {
                self.set_clip_muted(right, true);
            }
            self.set_clip_muted(clip_id, false);
        } else {
            // Covered in the middle: split out the covered section.
            if let Some(middle) = self.split_clip_at_sample(clip_id, overlap_start) {
                if let Some(tail) = self.split_clip_at_sample(middle, overlap_end) {
                    self.set_clip_muted(tail, false);
                }
                self.set_clip_muted(middle, true);
                self.set_clip_muted(clip_id, false);
            } else {
                self.set_clip_muted(clip_id, true);
            }
        }
    }

    fn apply_action(&mut self, action: &EditAction, forward: bool) {
        match action {
            EditAction::Move {
                clip_id,
                from_track,
                from_pos,
                to_track,
                to_pos,
            } => {
                let (track_id, position) = if forward {
                    (*to_track, *to_pos)
                } else {
                    (*from_track, *from_pos)
                };
                self.place_clip(*clip_id, track_id);
                if let Some(clip) = self.project.audio_clips.get_mut(*clip_id) {
                    clip.timeline_position = position;
                }
            }
            EditAction::Shape { clip_id, from, to } => {
                if let Some(clip) = self.project.audio_clips.get_mut(*clip_id) {
                    (if forward { *to } else { *from }).apply(clip);
                }
            }
            EditAction::Source { clip_id, from, to } => {
                let state = if forward { to } else { from };
                let mut missing_overview = None;
                if let Some(clip) = self.project.audio_clips.get_mut(*clip_id) {
                    state.apply(clip);
                    if clip.source.overview.is_none() {
                        missing_overview = Some(clip.source.path.clone());
                    }
                }
                if let Some(path) = missing_overview {
                    // The rendered file's overview may not exist on the undo
                    // entry yet (it is generated asynchronously).
                    self.spawn_overview(*clip_id, path);
                }
            }
            EditAction::Attach { track, clip_id } => {
                if forward {
                    self.attach_clip(*track, *clip_id);
                } else {
                    self.detach_clip(*track, *clip_id);
                }
            }
            EditAction::Detach { track, clip_id } => {
                if forward {
                    self.detach_clip(*track, *clip_id);
                } else {
                    self.attach_clip(*track, *clip_id);
                }
            }
            EditAction::TimeSignature { from, to } => {
                let (numerator, denominator) = if forward { *to } else { *from };
                self.project
                    .time_signature_map
                    .set_change(0, numerator, denominator);
            }
            EditAction::Tempo { from, to } => {
                self.project
                    .tempo_map
                    .set_default_bpm(if forward { *to } else { *from });
            }
            EditAction::TrackState { track_id, from, to } => {
                if let Some(track) = self.project.tracks.get_mut(track_id) {
                    (if forward { *to } else { *from }).apply(track);
                }
            }
            EditAction::TrackInserts { track_id, from, to } => {
                if let Some(track) = self.project.tracks.get_mut(track_id) {
                    track.inserts = if forward {
                        to.clone()
                    } else {
                        from.clone()
                    };
                }
            }
            EditAction::AddTrack { track } => {
                if forward {
                    self.project.add_track(track.clone());
                } else {
                    self.project.remove_track(track.id);
                }
            }
            EditAction::RemoveTrack { track, index } => {
                if forward {
                    self.project.remove_track(track.id);
                } else {
                    self.project.tracks.insert(track.id, track.clone());
                    if !self.project.track_order.contains(&track.id) {
                        let position = (*index).min(self.project.track_order.len());
                        self.project.track_order.insert(position, track.id);
                    }
                }
            }
            EditAction::MoveTrack {
                track_id,
                from_index,
                to_index,
            } => {
                let index = if forward { *to_index } else { *from_index };
                self.project.move_track(*track_id, index);
            }
            EditAction::AttachMidi { track, clip_id } => {
                if let Some(track) = self.project.tracks.get_mut(track) {
                    if forward {
                        track.add_midi_clip(*clip_id);
                    } else {
                        track.remove_midi_clip(*clip_id);
                    }
                }
            }
            EditAction::DetachMidi { track, clip_id } => {
                if let Some(track) = self.project.tracks.get_mut(track) {
                    if forward {
                        track.remove_midi_clip(*clip_id);
                    } else {
                        track.add_midi_clip(*clip_id);
                    }
                }
            }
            EditAction::MoveMidi {
                clip_id,
                from_track,
                from_pos,
                to_track,
                to_pos,
            } => {
                let (track_id, position) = if forward {
                    (*to_track, *to_pos)
                } else {
                    (*from_track, *from_pos)
                };
                self.place_midi_clip(*clip_id, track_id);
                if let Some(clip) = self.project.midi_clips.get_mut(*clip_id) {
                    clip.timeline_position = position;
                }
            }
            EditAction::MidiClip { clip_id, from, to } => {
                if let Some(clip) = self.project.midi_clips.get_mut(*clip_id) {
                    *clip = if forward {
                        to.clone()
                    } else {
                        from.clone()
                    };
                }
            }
            EditAction::AddMarker { marker } => {
                if forward {
                    if !self
                        .project
                        .marker_regions
                        .iter()
                        .any(|existing| existing.id == marker.id)
                    {
                        self.project.marker_regions.push(marker.clone());
                    }
                } else {
                    self.project
                        .marker_regions
                        .retain(|existing| existing.id != marker.id);
                }
            }
            EditAction::RemoveMarker { marker } => {
                if forward {
                    self.project
                        .marker_regions
                        .retain(|existing| existing.id != marker.id);
                } else if !self
                    .project
                    .marker_regions
                    .iter()
                    .any(|existing| existing.id == marker.id)
                {
                    self.project.marker_regions.push(marker.clone());
                }
            }
            EditAction::Group { actions, .. } => {
                if forward {
                    for action in actions {
                        self.apply_action(action, true);
                    }
                } else {
                    for action in actions.iter().rev() {
                        self.apply_action(action, false);
                    }
                }
            }
        }
    }

    fn undo(&mut self) {
        self.commit_pending_insert_edit();
        if self.undo_step() {
            self.after_edit();
            self.refresh_playback_if_playing();
            self.sync_monitoring();
            self.reload_piano_roll();
        }
    }

    fn redo(&mut self) {
        self.commit_pending_insert_edit();
        if self.redo_step() {
            self.after_edit();
            self.refresh_playback_if_playing();
            self.sync_monitoring();
            self.reload_piano_roll();
        }
    }

    /// Apply one undo step without refreshing playback. Returns whether an
    /// action was undone.
    fn undo_step(&mut self) -> bool {
        self.merge_chain = false;
        let Some(entry) = self.history.take_undo() else {
            return false;
        };
        self.apply_action(&entry.action, false);
        self.history.complete_undo(entry);
        true
    }

    /// Apply one redo step without refreshing playback.
    fn redo_step(&mut self) -> bool {
        self.merge_chain = false;
        let Some(entry) = self.history.take_redo() else {
            return false;
        };
        self.apply_action(&entry.action, true);
        self.history.complete_redo(entry);
        true
    }

    /// Jump the project to any point in the edit history.
    fn jump_history_to(&mut self, past_len: usize) {
        self.merge_chain = false;
        while self.history.past_len() > past_len && self.undo_step() {}
        while self.history.past_len() < past_len && self.redo_step() {}
        self.after_edit();
        self.refresh_playback_if_playing();
        self.sync_monitoring();
    }

    /// Hit-test a pointer position against drawn clips and choose the gesture
    /// for that spot: fade handle, trim edge or move.
    fn clip_hit(
        clip_rects: &[(ClipId, egui::Rect)],
        pos: egui::Pos2,
    ) -> Option<(ClipId, DragMode)> {
        for (clip_id, rect) in clip_rects.iter().rev() {
            if !rect.contains(pos) {
                continue;
            }
            let wide = rect.width() > FADE_HANDLE_PX * 4.0;
            let top = pos.y <= rect.top() + FADE_HANDLE_PX;
            let mode = if wide && top && pos.x <= rect.left() + FADE_HANDLE_PX {
                DragMode::FadeIn
            } else if wide && top && pos.x >= rect.right() - FADE_HANDLE_PX {
                DragMode::FadeOut
            } else if pos.x <= rect.left() + CLIP_EDGE_PX {
                DragMode::TrimStart
            } else if pos.x >= rect.right() - CLIP_EDGE_PX {
                DragMode::TrimEnd
            } else {
                DragMode::Move
            };
            return Some((*clip_id, mode));
        }
        None
    }

    /// MIDI clips only expose trim handles for now: the left and right edge
    /// zones. The body returns `Move`, but callers currently ignore it.
    fn midi_clip_hit(
        clip_rects: &[(ClipId, egui::Rect)],
        pos: egui::Pos2,
    ) -> Option<(ClipId, DragMode)> {
        for (clip_id, rect) in clip_rects.iter().rev() {
            if !rect.contains(pos) {
                continue;
            }
            let mode = if pos.x <= rect.left() + CLIP_EDGE_PX {
                DragMode::TrimStart
            } else if pos.x >= rect.right() - CLIP_EDGE_PX {
                DragMode::TrimEnd
            } else {
                DragMode::Move
            };
            return Some((*clip_id, mode));
        }
        None
    }

    fn track_at_y(&self, y: f32, track_y_start: f32, track_height: f32) -> Option<TrackId> {
        if self.project.track_order.is_empty() || track_height <= 0.0 {
            return None;
        }
        let index = (((y - track_y_start) / track_height).floor() as i64)
            .clamp(0, self.project.track_order.len() as i64 - 1) as usize;
        self.project.track_order.get(index).copied()
    }

    fn track_index(&self, track_id: TrackId) -> Option<usize> {
        self.project.track_order.iter().position(|id| *id == track_id)
    }

    fn begin_clip_drag(
        &mut self,
        pos: egui::Pos2,
        clip_rects: &[(ClipId, egui::Rect)],
        rect: egui::Rect,
        view: TimelineView,
    ) {
        let Some((clip_id, mode)) = Self::clip_hit(clip_rects, pos) else {
            return;
        };
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked {
            return;
        }
        let Some(track_id) = self.clip_track(clip_id) else {
            return;
        };
        // Don't start a clip drag on top of the playhead handle; that gesture
        // belongs to the playhead.
        let playhead_x = view.x(rect, self.playhead_secs);
        if (pos.x - playhead_x).abs() <= CLIP_EDGE_PX {
            return;
        }
        let project_sr = self.project.sample_rate.max(1) as f64;
        let clip_start = clip.timeline_position.sample as f64 / project_sr;
        let from_shape = ClipShape::of(clip);

        if self.selected_clips.contains(&clip_id) {
            self.selected_clip = Some(clip_id);
        } else {
            self.set_single_selection(clip_id);
        }

        // Body drags move the whole selection; edge/fade drags stay on one clip.
        let mut members = Vec::new();
        if mode == DragMode::Move {
            for other in self.selected_clips.iter().copied() {
                if other == clip_id {
                    continue;
                }
                let Some(other_clip) = self.project.audio_clips.get(other) else {
                    continue;
                };
                if other_clip.locked {
                    continue;
                }
                let Some(other_track) = self.clip_track(other) else {
                    continue;
                };
                members.push(ClipDragMember {
                    clip_id: other,
                    from_shape: ClipShape::of(other_clip),
                    from_track: other_track,
                    last_track: other_track,
                });
            }
        }

        let mut midi_members = Vec::new();
        if mode == DragMode::Move {
            for midi_id in self.midi_selection_ids() {
                let Some(midi_track) = self.midi_clip_track(midi_id) else {
                    continue;
                };
                let Some(midi_clip) = self.project.midi_clips.get(midi_id) else {
                    continue;
                };
                midi_members.push(MidiMoveMember {
                    clip_id: midi_id,
                    from_track: midi_track,
                    from_pos: midi_clip.timeline_position,
                    last_track: midi_track,
                });
            }
        }

        self.drag = Some(ClipDrag {
            clip_id,
            mode,
            from_shape,
            from_track: track_id,
            last_track: track_id,
            grab_offset_secs: (view.t(rect, pos.x) - clip_start).max(0.0),
            members,
            midi_members,
        });
    }

    fn update_clip_drag(
        &mut self,
        pos: egui::Pos2,
        rect: egui::Rect,
        track_y_start: f32,
        track_height: f32,
        view: TimelineView,
    ) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        let Some(clip) = self.project.audio_clips.get(drag.clip_id) else {
            self.drag = Some(drag);
            return;
        };

        let project_sr = self.project.sample_rate.max(1) as f64;
        let src_sr = clip.source.sample_rate.max(1) as f64;
        let source_frames = clip.source.duration_samples;
        let bpm = self.project.tempo_map.default_bpm();
        let pointer_t = view.t(rect, pos.x);
        let mut shape = drag.from_shape;

        match drag.mode {
            DragMode::Move => {
                let raw_start = (pointer_t - drag.grab_offset_secs).max(0.0);
                let start = match (self.snap_relative, self.snap_grid_secs()) {
                    (true, Some(grid)) => {
                        // Relative snap: move in grid-sized steps from the
                        // clip's original position, preserving any off-grid
                        // offset instead of pulling it onto the absolute grid.
                        let anchor =
                            drag.from_shape.position.sample as f64 / project_sr;
                        (anchor + ((raw_start - anchor) / grid).round() * grid).max(0.0)
                    }
                    _ => self.snap_secs(raw_start),
                };
                shape.position = TimelinePos::from_sample(
                    (start * project_sr).round() as u64,
                    project_sr,
                    bpm,
                );
                let from_index = self
                    .project
                    .track_order
                    .iter()
                    .position(|id| *id == drag.from_track);
                let raw_target_index = self
                    .track_at_y(pos.y, track_y_start, track_height)
                    .and_then(|track_id| {
                        self.project
                            .track_order
                            .iter()
                            .position(|id| *id == track_id)
                    });
                if let (Some(from_index), Some(raw_target_index)) =
                    (from_index, raw_target_index)
                {
                    // Clamp the vertical delta so the whole group stays inside
                    // the track list instead of piling up at the edges.
                    let last_index = self.project.track_order.len().saturating_sub(1) as i64;
                    let mut min_index = from_index as i64;
                    let mut max_index = from_index as i64;
                    for member in &drag.members {
                        if let Some(index) = self
                            .project
                            .track_order
                            .iter()
                            .position(|id| *id == member.from_track)
                        {
                            min_index = min_index.min(index as i64);
                            max_index = max_index.max(index as i64);
                        }
                    }
                    for member in &drag.midi_members {
                        if let Some(index) = self.track_index(member.from_track) {
                            min_index = min_index.min(index as i64);
                            max_index = max_index.max(index as i64);
                        }
                    }
                    let raw_delta = raw_target_index as i64 - from_index as i64;
                    let mut track_delta =
                        raw_delta.clamp(-min_index, last_index - max_index);
                    // MIDI clips can only land on MIDI/instrument tracks. If a
                    // mixed selection would put one somewhere invalid, keep the
                    // whole group on its current tracks and move in time only.
                    let midi_valid = drag.midi_members.iter().all(|member| {
                        let Some(member_index) =
                            self.track_index(member.from_track)
                        else {
                            return false;
                        };
                        let target_index = (member_index as i64 + track_delta)
                            .clamp(0, last_index)
                            as usize;
                        self.project
                            .track_order
                            .get(target_index)
                            .and_then(|id| self.project.tracks.get(id))
                            .map(|track| {
                                matches!(
                                    track.track_type,
                                    TrackType::Midi | TrackType::Instrument
                                )
                            })
                            .unwrap_or(false)
                    });
                    if !midi_valid {
                        track_delta = 0;
                    }
                    let target_index = (from_index as i64 + track_delta) as usize;
                    if let Some(target_track) = self.project.track_order.get(target_index).copied()
                    {
                        if target_track != drag.last_track {
                            self.detach_clip(drag.last_track, drag.clip_id);
                            self.attach_clip(target_track, drag.clip_id);
                            drag.last_track = target_track;
                        }
                    }
                }

                // Move every other selected clip by the same time delta,
                // preserving their relative track offsets. Clamp the delta so
                // that the earliest clip in the group stops at the timeline
                // start instead of later clips compressing into it.
                let min_start = drag
                    .members
                    .iter()
                    .map(|member| member.from_shape.position.sample)
                    .chain(drag.midi_members.iter().map(|member| {
                        self.project
                            .tempo_map
                            .tick_to_sample(member.from_pos.tick, project_sr)
                    }))
                    .chain(std::iter::once(drag.from_shape.position.sample))
                    .min()
                    .unwrap_or(0);
                let raw_delta = shape.position.sample as i64
                    - drag.from_shape.position.sample as i64;
                let delta_samples = clamp_group_delta(min_start, raw_delta);
                if delta_samples != raw_delta {
                    shape.position = TimelinePos::from_sample(
                        (drag.from_shape.position.sample as i64 + delta_samples).max(0)
                            as u64,
                        project_sr,
                        bpm,
                    );
                }
                let from_index = self
                    .project
                    .track_order
                    .iter()
                    .position(|id| *id == drag.from_track);
                let to_index = self
                    .project
                    .track_order
                    .iter()
                    .position(|id| *id == drag.last_track);
                let track_delta = match (from_index, to_index) {
                    (Some(from), Some(to)) => to as i64 - from as i64,
                    _ => 0,
                };

                for member in drag.members.iter_mut() {
                    let new_sample =
                        (member.from_shape.position.sample as i64 + delta_samples).max(0) as u64;
                    if let Some(clip) = self.project.audio_clips.get_mut(member.clip_id) {
                        clip.timeline_position =
                            TimelinePos::from_sample(new_sample, project_sr, bpm);
                    }
                    if let Some(member_from_index) = self
                        .project
                        .track_order
                        .iter()
                        .position(|id| *id == member.from_track)
                    {
                        let last_index = self.project.track_order.len().saturating_sub(1) as i64;
                        let target_index =
                            (member_from_index as i64 + track_delta).clamp(0, last_index)
                                as usize;
                        if let Some(target_track) =
                            self.project.track_order.get(target_index).copied()
                        {
                            if target_track != member.last_track {
                                self.detach_clip(member.last_track, member.clip_id);
                                self.attach_clip(target_track, member.clip_id);
                                member.last_track = target_track;
                            }
                        }
                    }
                }

                for member in drag.midi_members.iter_mut() {
                    let from_sample = self
                        .project
                        .tempo_map
                        .tick_to_sample(member.from_pos.tick, project_sr);
                    let new_sample =
                        (from_sample as i64 + delta_samples).max(0) as u64;
                    let new_tick = self
                        .project
                        .tempo_map
                        .sample_to_tick(new_sample, project_sr);
                    if let Some(clip) = self.project.midi_clips.get_mut(member.clip_id)
                    {
                        clip.timeline_position.tick = new_tick;
                        clip.timeline_position.sample = new_sample;
                    }
                    if let Some(member_from_index) =
                        self.track_index(member.from_track)
                    {
                        let last_index = self
                            .project
                            .track_order
                            .len()
                            .saturating_sub(1)
                            as i64;
                        let target_index =
                            (member_from_index as i64 + track_delta)
                                .clamp(0, last_index)
                                as usize;
                        if let Some(target_track) = self
                            .project
                            .track_order
                            .get(target_index)
                            .copied()
                        {
                            if target_track != member.last_track {
                                self.place_midi_clip(member.clip_id, target_track);
                                member.last_track = target_track;
                            }
                        }
                    }
                }
            }
            DragMode::TrimStart => {
                let clip_start = drag.from_shape.position.sample as f64 / project_sr;
                let clip_end = clip_start + drag.from_shape.length as f64 / src_sr;
                // Trimming left reveals hidden source material before the clip;
                // it can never extend past the file or the clip's end.
                let earliest =
                    (clip_start - drag.from_shape.source_offset as f64 / src_sr).max(0.0);
                let latest = (clip_end - MIN_CLIP_SECS).max(earliest);
                let new_start = self.snap_secs(pointer_t).clamp(earliest, latest);
                let delta_frames = ((new_start - clip_start) * src_sr).round() as i64;
                let new_offset = drag.from_shape.source_offset as i64 + delta_frames;
                let new_length = drag.from_shape.length as i64 - delta_frames;
                if new_offset >= 0 && new_length > 0 {
                    shape.source_offset = new_offset as u64;
                    shape.length = new_length as u64;
                    shape.position = TimelinePos::from_sample(
                        (new_start * project_sr).round() as u64,
                        project_sr,
                        bpm,
                    );
                }
            }
            DragMode::TrimEnd => {
                let clip_start = drag.from_shape.position.sample as f64 / project_sr;
                let available = source_frames.saturating_sub(drag.from_shape.source_offset);
                let max_end = clip_start + available as f64 / src_sr;
                let min_end = clip_start + MIN_CLIP_SECS;
                let new_end = self.snap_secs(pointer_t).clamp(min_end, max_end.max(min_end));
                shape.length = (((new_end - clip_start) * src_sr).round() as u64).max(1);
            }
            DragMode::FadeIn => {
                let clip_start = drag.from_shape.position.sample as f64 / project_sr;
                let frames = ((pointer_t - clip_start).max(0.0) * src_sr).round() as u64;
                shape.fade_in =
                    make_fade(frames, drag.from_shape.fade_in, drag.from_shape.length);
            }
            DragMode::FadeOut => {
                let clip_start = drag.from_shape.position.sample as f64 / project_sr;
                let clip_end = clip_start + drag.from_shape.length as f64 / src_sr;
                let frames = ((clip_end - pointer_t).max(0.0) * src_sr).round() as u64;
                shape.fade_out =
                    make_fade(frames, drag.from_shape.fade_out, drag.from_shape.length);
            }
        }

        if let Some(clip) = self.project.audio_clips.get_mut(drag.clip_id) {
            shape.apply(clip);
        }
        self.drag = Some(drag);
    }

    fn finish_clip_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let Some(clip) = self.project.audio_clips.get(drag.clip_id) else {
            return;
        };
        let current = ClipShape::of(clip);

        match drag.mode {
            DragMode::Move => {
                let mut actions = Vec::new();
                if drag.last_track != drag.from_track
                    || current.position != drag.from_shape.position
                {
                    actions.push(EditAction::Move {
                        clip_id: drag.clip_id,
                        from_track: drag.from_track,
                        from_pos: drag.from_shape.position,
                        to_track: drag.last_track,
                        to_pos: current.position,
                    });
                }
                for member in &drag.members {
                    let Some(clip) = self.project.audio_clips.get(member.clip_id) else {
                        continue;
                    };
                    if member.last_track != member.from_track
                        || clip.timeline_position != member.from_shape.position
                    {
                        actions.push(EditAction::Move {
                            clip_id: member.clip_id,
                            from_track: member.from_track,
                            from_pos: member.from_shape.position,
                            to_track: member.last_track,
                            to_pos: clip.timeline_position,
                        });
                    }
                }
                for member in &drag.midi_members {
                    let Some(clip) = self.project.midi_clips.get(member.clip_id) else {
                        continue;
                    };
                    if member.last_track != member.from_track
                        || clip.timeline_position != member.from_pos
                    {
                        actions.push(EditAction::MoveMidi {
                            clip_id: member.clip_id,
                            from_track: member.from_track,
                            from_pos: member.from_pos,
                            to_track: member.last_track,
                            to_pos: clip.timeline_position,
                        });
                    }
                }
                if !actions.is_empty() {
                    if actions.len() == 1 {
                        self.push_undo(actions.pop().unwrap(), false);
                    } else {
                        self.push_undo(
                            EditAction::Group {
                                actions,
                                label: "Move clips".to_string(),
                            },
                            false,
                        );
                    }
                    self.after_edit();
                    self.refresh_playback_if_playing();
                }
            }
            _ => {
                if current != drag.from_shape {
                    self.push_undo(
                        EditAction::Shape {
                            clip_id: drag.clip_id,
                            from: drag.from_shape,
                            to: current,
                        },
                        false,
                    );
                    self.after_edit();
                    self.refresh_playback_if_playing();
                }
            }
        }
    }

    /// Start a MIDI clip trim from the left or right edge.
    fn begin_midi_clip_drag(
        &mut self,
        pos: egui::Pos2,
        midi_rects: &[(ClipId, egui::Rect)],
        rect: egui::Rect,
        view: TimelineView,
    ) {
        let Some((clip_id, mode)) = Self::midi_clip_hit(midi_rects, pos) else {
            return;
        };
        if !matches!(
            mode,
            DragMode::Move | DragMode::TrimStart | DragMode::TrimEnd
        ) {
            return;
        }
        let Some(before) = self.project.midi_clips.get(clip_id).cloned() else {
            return;
        };
        let Some(from_track) = self.midi_clip_track(clip_id) else {
            return;
        };
        // Don't start a trim on top of the playhead handle.
        let playhead_x = view.x(rect, self.playhead_secs);
        if (pos.x - playhead_x).abs() <= CLIP_EDGE_PX {
            return;
        }
        let project_sr = self.project.sample_rate.max(1) as f64;
        let pointer_sample = (view.t(rect, pos.x).max(0.0) * project_sr).round() as u64;
        let pointer_tick = self
            .project
            .tempo_map
            .sample_to_tick(pointer_sample, project_sr);
        let grab_offset_ticks =
            pointer_tick as i64 - before.timeline_position.tick as i64;
        let selected = if self.selected_midi_clips.contains(&clip_id) {
            self.selected_midi_clip = Some(clip_id);
            self.midi_selection_ids()
        } else {
            self.set_single_midi_selection(clip_id);
            vec![clip_id]
        };
        let mut members = Vec::new();
        if mode == DragMode::Move {
            for member_id in selected {
                let Some(track_id) = self.midi_clip_track(member_id) else {
                    continue;
                };
                let Some(member_clip) = self.project.midi_clips.get(member_id) else {
                    continue;
                };
                members.push(MidiMoveMember {
                    clip_id: member_id,
                    from_track: track_id,
                    from_pos: member_clip.timeline_position,
                    last_track: track_id,
                });
            }
        }

        let mut audio_members = Vec::new();
        if mode == DragMode::Move {
            for audio_id in self.selection_ids() {
                let Some(track_id) = self.clip_track(audio_id) else {
                    continue;
                };
                let Some(audio_clip) = self.project.audio_clips.get(audio_id) else {
                    continue;
                };
                if audio_clip.locked {
                    continue;
                }
                audio_members.push(ClipDragMember {
                    clip_id: audio_id,
                    from_shape: ClipShape::of(audio_clip),
                    from_track: track_id,
                    last_track: track_id,
                });
            }
        }
        self.midi_drag = Some(MidiClipDrag {
            clip_id,
            mode,
            before,
            from_track,
            last_track: from_track,
            grab_offset_ticks,
            members,
            audio_members,
        });
    }

    /// Move a MIDI clip in time/across tracks, or trim its visible window.
    fn update_midi_clip_drag(
        &mut self,
        pos: egui::Pos2,
        rect: egui::Rect,
        track_y_start: f32,
        track_height: f32,
        view: TimelineView,
    ) {
        let Some(mut drag) = self.midi_drag.take() else {
            return;
        };
        let before = drag.before.clone();
        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        let beats_per_bar = self.time_signature().0.max(1) as u64;
        let pointer_sample = (view.t(rect, pos.x).max(0.0) * project_sr).round() as u64;
        let pointer_tick = self
            .project
            .tempo_map
            .sample_to_tick(pointer_sample, project_sr);
        let snap_ticks = timeline_snap_ticks(self.snap_mode, beats_per_bar);
        let snap_tick = |tick: u64| -> u64 {
            if snap_ticks <= 1 {
                tick
            } else {
                ((tick + snap_ticks / 2) / snap_ticks) * snap_ticks
            }
        };

        match drag.mode {
            DragMode::Move => {
                let raw_start = (pointer_tick as i64 - drag.grab_offset_ticks).max(0) as u64;
                let new_start = if snap_ticks <= 1 {
                    raw_start
                } else if self.snap_relative {
                    let grid = snap_ticks as f64;
                    let anchor = before.timeline_position.tick as f64;
                    let steps = ((raw_start as f64 - anchor) / grid).round();
                    (anchor + steps * grid).max(0.0) as u64
                } else {
                    snap_tick(raw_start)
                };
                let from_sample = self
                    .project
                    .tempo_map
                    .tick_to_sample(before.timeline_position.tick, project_sr);
                let target_sample = self
                    .project
                    .tempo_map
                    .tick_to_sample(new_start, project_sr);
                let mut delta_samples =
                    target_sample as i64 - from_sample as i64;
                let mut min_start = u64::MAX;
                for member in &drag.members {
                    min_start = min_start.min(
                        self.project
                            .tempo_map
                            .tick_to_sample(member.from_pos.tick, project_sr),
                    );
                }
                for member in &drag.audio_members {
                    min_start = min_start.min(member.from_shape.position.sample);
                }
                if min_start != u64::MAX {
                    delta_samples = clamp_group_delta(min_start, delta_samples);
                }

                // Compute a shared vertical delta, but only if every selected
                // clip can land on a MIDI/instrument track. Otherwise move the
                // group in time only.
                let mut track_delta = 0i64;
                if let Some(target_id) =
                    self.track_at_y(pos.y, track_y_start, track_height)
                {
                    let target_valid = self
                        .project
                        .tracks
                        .get(&target_id)
                        .map(|track| {
                            matches!(
                                track.track_type,
                                TrackType::Midi | TrackType::Instrument
                            )
                        })
                        .unwrap_or(false);
                    if target_valid {
                        if let (Some(from_index), Some(target_index)) = (
                            self.track_index(drag.from_track),
                            self.track_index(target_id),
                        ) {
                            let last_index = self
                                .project
                                .track_order
                                .len()
                                .saturating_sub(1)
                                as i64;
                            let mut min_index = from_index as i64;
                            let mut max_index = from_index as i64;
                            for member in &drag.members {
                                if let Some(index) =
                                    self.track_index(member.from_track)
                                {
                                    min_index = min_index.min(index as i64);
                                    max_index = max_index.max(index as i64);
                                }
                            }
                            for member in &drag.audio_members {
                                if let Some(index) =
                                    self.track_index(member.from_track)
                                {
                                    min_index = min_index.min(index as i64);
                                    max_index = max_index.max(index as i64);
                                }
                            }
                            let candidate = (target_index as i64 - from_index as i64)
                                .clamp(-min_index, last_index - max_index);
                            let all_valid = drag.members.iter().all(|member| {
                                let Some(from_index) =
                                    self.track_index(member.from_track)
                                else {
                                    return false;
                                };
                                let index = (from_index as i64 + candidate)
                                    .clamp(0, last_index)
                                    as usize;
                                self.project
                                    .track_order
                                    .get(index)
                                    .and_then(|id| self.project.tracks.get(id))
                                    .map(|track| {
                                        matches!(
                                            track.track_type,
                                            TrackType::Midi | TrackType::Instrument
                                        )
                                    })
                                    .unwrap_or(false)
                            });
                            if all_valid {
                                track_delta = candidate;
                            }
                        }
                    }
                }

                for member in &mut drag.members {
                    let member_from_sample = self
                        .project
                        .tempo_map
                        .tick_to_sample(member.from_pos.tick, project_sr);
                    let new_sample =
                        (member_from_sample as i64 + delta_samples).max(0) as u64;
                    let new_tick = self
                        .project
                        .tempo_map
                        .sample_to_tick(new_sample, project_sr);
                    if let Some(clip) =
                        self.project.midi_clips.get_mut(member.clip_id)
                    {
                        clip.timeline_position.tick = new_tick;
                        clip.timeline_position.sample = new_sample;
                    }
                    if track_delta != 0 {
                        if let Some(from_index) = self.track_index(member.from_track) {
                            let last_index = self
                                .project
                                .track_order
                                .len()
                                .saturating_sub(1)
                                as i64;
                            let target_index = (from_index as i64 + track_delta)
                                .clamp(0, last_index)
                                as usize;
                            if let Some(target_track) = self
                                .project
                                .track_order
                                .get(target_index)
                                .copied()
                            {
                                if target_track != member.last_track {
                                    self.place_midi_clip(member.clip_id, target_track);
                                    member.last_track = target_track;
                                }
                            }
                        }
                    }
                    if member.clip_id == drag.clip_id {
                        drag.last_track = member.last_track;
                    }
                }

                for member in drag.audio_members.iter_mut() {
                    let new_sample = (member.from_shape.position.sample as i64
                        + delta_samples)
                        .max(0) as u64;
                    if let Some(clip) =
                        self.project.audio_clips.get_mut(member.clip_id)
                    {
                        clip.timeline_position =
                            TimelinePos::from_sample(new_sample, project_sr, bpm);
                    }
                    if track_delta != 0 {
                        if let Some(from_index) = self.track_index(member.from_track) {
                            let last_index = self
                                .project
                                .track_order
                                .len()
                                .saturating_sub(1)
                                as i64;
                            let target_index = (from_index as i64 + track_delta)
                                .clamp(0, last_index)
                                as usize;
                            if let Some(target_track) = self
                                .project
                                .track_order
                                .get(target_index)
                                .copied()
                            {
                                if target_track != member.last_track {
                                    self.detach_clip(member.last_track, member.clip_id);
                                    self.attach_clip(target_track, member.clip_id);
                                    member.last_track = target_track;
                                }
                            }
                        }
                    }
                }
            }
            DragMode::TrimStart => {
                let (new_start, new_offset, new_length) =
                    midi_trim_start(&before, pointer_tick, snap_ticks);
                let new_sample = self
                    .project
                    .tempo_map
                    .tick_to_sample(new_start, project_sr);
                if let Some(clip) = self.project.midi_clips.get_mut(drag.clip_id) {
                    clip.timeline_position.tick = new_start;
                    clip.timeline_position.sample = new_sample;
                    clip.content_offset = new_offset;
                    clip.length = new_length;
                }
            }
            DragMode::TrimEnd => {
                let new_length = midi_trim_end(&before, pointer_tick, snap_ticks);
                if let Some(clip) = self.project.midi_clips.get_mut(drag.clip_id) {
                    clip.length = new_length;
                }
            }
            _ => {}
        }

        self.midi_drag = Some(drag);
        self.refresh_midi_playback();
    }

    /// Commit a finished MIDI move/trim as one undo step.
    fn finish_midi_clip_drag(&mut self) {
        let Some(drag) = self.midi_drag.take() else {
            return;
        };
        let Some(after) = self.project.midi_clips.get(drag.clip_id).cloned() else {
            return;
        };
        if drag.mode == DragMode::Move {
            let mut actions = Vec::new();
            for member in &drag.members {
                let Some(clip) = self.project.midi_clips.get(member.clip_id) else {
                    continue;
                };
                if member.last_track != member.from_track
                    || clip.timeline_position != member.from_pos
                {
                    actions.push(EditAction::MoveMidi {
                        clip_id: member.clip_id,
                        from_track: member.from_track,
                        from_pos: member.from_pos,
                        to_track: member.last_track,
                        to_pos: clip.timeline_position,
                    });
                }
            }
            for member in &drag.audio_members {
                let Some(clip) = self.project.audio_clips.get(member.clip_id) else {
                    continue;
                };
                if member.last_track != member.from_track
                    || clip.timeline_position != member.from_shape.position
                {
                    actions.push(EditAction::Move {
                        clip_id: member.clip_id,
                        from_track: member.from_track,
                        from_pos: member.from_shape.position,
                        to_track: member.last_track,
                        to_pos: clip.timeline_position,
                    });
                }
            }
            if !actions.is_empty() {
                if actions.len() == 1 {
                    self.push_undo(actions.pop().unwrap(), false);
                } else {
                    self.push_undo(
                        EditAction::Group {
                            actions,
                            label: "Move clips".to_string(),
                        },
                        false,
                    );
                }
                self.after_edit();
                self.refresh_playback_if_playing();
            }
        } else if after != drag.before {
            self.push_undo(
                EditAction::MidiClip {
                    clip_id: drag.clip_id,
                    from: drag.before,
                    to: after,
                },
                false,
            );
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    /// Split the selected clip at the playhead.
    fn split_selected_clip(&mut self) {
        let midi_id = self.selected_midi_clip;
        let audio_id = self.selected_clip;
        if midi_id.is_none() && audio_id.is_none() {
            return;
        }
        if let Some(midi_id) = midi_id {
            self.split_midi_clip_at(midi_id, self.playhead_secs);
        }
        if let Some(audio_id) = audio_id {
            self.split_clip_at(audio_id, self.playhead_secs);
        }
    }

    /// Split a MIDI clip at `split_secs` into two non-destructive halves.
    fn split_midi_clip_at(&mut self, clip_id: ClipId, split_secs: f64) -> Option<ClipId> {
        let track_id = self.midi_clip_track(clip_id)?;
        let clip = self.project.midi_clips.get(clip_id)?.clone();
        let project_sr = self.project.sample_rate.max(1) as f64;
        let sample = (split_secs.max(0.0) * project_sr).round() as u64;
        let split_tick = self.project.tempo_map.sample_to_tick(sample, project_sr);
        let Some((left, mut right)) = split_midi_clip_at_tick(&clip, split_tick) else {
            self.error_message =
                Some("The split point must be inside the MIDI clip.".to_string());
            return None;
        };

        right.timeline_position.sample =
            self.project.tempo_map.tick_to_sample(split_tick, project_sr);
        let right_id = self.project.midi_clips.insert(right);
        let action = EditAction::Group {
            actions: vec![
                EditAction::MidiClip {
                    clip_id,
                    from: clip,
                    to: left,
                },
                EditAction::AttachMidi {
                    track: track_id,
                    clip_id: right_id,
                },
            ],
            label: "Split MIDI clip".to_string(),
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
        self.refresh_playback_if_playing();
        Some(right_id)
    }

    /// Split `clip_id` at `split_secs` into two clips referencing the same
    /// source file. The audio file itself is never modified.
    fn split_clip_at(&mut self, clip_id: ClipId, split_secs: f64) -> Option<ClipId> {
        let track_id = self.clip_track(clip_id)?;

        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        let clip = self.project.audio_clips.get(clip_id)?;
        if clip.locked {
            return None;
        }
        let src_sr = clip.source.sample_rate.max(1) as f64;
        let clip_start = clip.timeline_position.sample as f64 / project_sr;
        let clip_end = clip_start + clip.length as f64 / src_sr;

        if split_secs <= clip_start + MIN_CLIP_SECS || split_secs >= clip_end - MIN_CLIP_SECS {
            self.error_message =
                Some("The split point must be inside the clip.".to_string());
            return None;
        }

        let from_shape = ClipShape::of(clip);
        let offset_frames = ((split_secs - clip_start) * src_sr).round().max(1.0) as u64;
        if clip.length <= offset_frames + 1 {
            return None;
        }

        let mut right = clip.clone();
        right.id = ClipId::default();
        right.source_offset = clip.source_offset + offset_frames;
        right.timeline_position = TimelinePos::from_sample(
            (split_secs * project_sr).round() as u64,
            project_sr,
            bpm,
        );
        right.length = clip.length - offset_frames;
        right.fade_in = None;
        right.fade_out = from_shape
            .fade_out
            .map(|fade| Fade::new(fade.length.min(right.length), fade.curve));

        let mut to_shape = from_shape;
        to_shape.length = offset_frames;
        to_shape.fade_out = None;
        to_shape.fade_in = from_shape
            .fade_in
            .map(|fade| Fade::new(fade.length.min(offset_frames), fade.curve));

        let new_id = self.project.audio_clips.insert(right);
        let action = EditAction::Group {
            actions: vec![
                EditAction::Shape {
                    clip_id,
                    from: from_shape,
                    to: to_shape,
                },
                EditAction::Attach {
                    track: track_id,
                    clip_id: new_id,
                },
            ],
            label: "Split clip".to_string(),
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
        self.refresh_playback_if_playing();
        Some(new_id)
    }

    /// Split at a project-sample position.
    fn split_clip_at_sample(&mut self, clip_id: ClipId, sample: u64) -> Option<ClipId> {
        let secs = sample as f64 / self.project.sample_rate.max(1) as f64;
        self.split_clip_at(clip_id, secs)
    }

    /// Delete the selected clip from its track. The clip data stays in the
    /// project pool, so the delete is recoverable (trash + undo).
    fn delete_selected_clip(&mut self) {
        let midi_ids = self.midi_selection_ids();
        let audio_ids = self.selection_ids();
        if midi_ids.is_empty() && audio_ids.is_empty() {
            return;
        }

        let mut actions = Vec::new();
        for midi_id in midi_ids {
            if let Some(track_id) = self.midi_clip_track(midi_id) {
                actions.push(EditAction::DetachMidi {
                    track: track_id,
                    clip_id: midi_id,
                });
            }
        }
        for clip_id in audio_ids {
            let Some(track_id) = self.clip_track(clip_id) else {
                continue;
            };
            if self
                .project
                .audio_clips
                .get(clip_id)
                .map(|clip| clip.locked)
                .unwrap_or(false)
            {
                continue;
            }
            actions.push(EditAction::Detach {
                track: track_id,
                clip_id,
            });
        }
        if actions.is_empty() {
            return;
        }
        let label = if actions.len() == 1 {
            "Delete clip".to_string()
        } else {
            format!("Delete {} clips", actions.len())
        };
        let action = if actions.len() == 1 {
            actions.pop().unwrap()
        } else {
            EditAction::Group { actions, label }
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.clear_selection();
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Put a clip that was removed from its track back into the arrangement.
    fn restore_clip(&mut self, clip_id: ClipId) {
        let track_id = self
            .deleted_from
            .get(&clip_id)
            .copied()
            .filter(|id| self.project.tracks.contains_key(id))
            .or_else(|| self.project.track_order.first().copied());
        let Some(track_id) = track_id else {
            self.error_message = Some("No track available to restore the clip.".to_string());
            return;
        };

        let action = EditAction::Attach { track: track_id, clip_id };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.set_single_selection(clip_id);
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Peak absolute sample in the clip's visible source range. Seeks to the
    /// range and streams it, so long files are not loaded into memory.
    fn clip_source_peak(clip: &AudioClip) -> Option<f32> {
        wav_peak(&clip.source.path, clip.source_offset, clip.length).ok()
    }

    /// Set the clip gain so the loudest sample in its visible range peaks at
    /// -1 dBFS. Non-destructive: only the gain changes.
    fn normalize_clip(&mut self, clip_id: ClipId) {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked {
            return;
        }
        let Some(peak) = Self::clip_source_peak(clip) else {
            self.error_message =
                Some("Could not read the clip's audio for normalization.".to_string());
            return;
        };
        if peak <= 1e-6 {
            self.error_message = Some("The clip is silent; nothing to normalize.".to_string());
            return;
        }

        let target = 10.0f32.powf(-1.0 / 20.0);
        let before = ClipShape::of(clip);
        let mut after = before;
        after.gain = (target / peak).clamp(0.0, 10.0);
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            after.apply(clip);
        }
        self.push_undo(
            EditAction::Shape {
                clip_id,
                from: before,
                to: after,
            },
            false,
        );
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Set the clip gain from its integrated loudness, capped so the sample
    /// peak stays at or below −1 dBFS.
    fn lufs_normalize_clip(&mut self, clip_id: ClipId, target_lufs: f32) {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked {
            return;
        }

        let (samples, channels, sample_rate) =
            match load_wav_range_interleaved(&clip.source.path, clip.source_offset, clip.length) {
                Ok(data) => data,
                Err(e) => {
                    self.error_message = Some(format!(
                        "Could not read the clip's audio for loudness analysis: {}",
                        e
                    ));
                    return;
                }
            };
        let Some(measurement) = measure_integrated_loudness(&samples, channels, sample_rate) else {
            self.error_message =
                Some("The clip is too quiet or too short to measure loudness.".to_string());
            return;
        };

        let mut gain_db = target_lufs - measurement.integrated_lufs;
        if measurement.peak > 1e-6 {
            let peak_db = 20.0 * measurement.peak.log10();
            gain_db = gain_db.min(-1.0 - peak_db);
        }

        let before = ClipShape::of(clip);
        let mut after = before;
        after.gain = 10.0f32.powf(gain_db / 20.0).clamp(0.0, 10.0);
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            after.apply(clip);
        }
        self.push_undo(
            EditAction::Shape {
                clip_id,
                from: before,
                to: after,
            },
            false,
        );
        self.error_message = None;
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Render the clip's visible audio backwards into a new WAV file and point
    /// the clip at it. The original file is never modified.
    fn reverse_clip(&mut self, clip_id: ClipId) {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked || clip.length == 0 {
            return;
        }

        let (samples, channels, sample_rate) = match load_wav_interleaved(&clip.source.path) {
            Ok(data) => data,
            Err(e) => {
                self.error_message = Some(format!("Cannot read clip audio: {}", e));
                return;
            }
        };
        let channel_count = channels.max(1) as usize;
        let total_frames = samples.len() / channel_count;
        let first_frame = (clip.source_offset as usize).min(total_frames);
        let frame_count = (clip.length as usize).min(total_frames - first_frame);
        if frame_count == 0 {
            self.error_message = Some("The clip has no audio to reverse.".to_string());
            return;
        }

        let start = first_frame * channel_count;
        let end = start + frame_count * channel_count;
        let reversed = reverse_interleaved(&samples[start..end], channel_count);

        let parent = clip.source.path.parent().unwrap_or_else(|| Path::new(""));
        let stem = processed_file_stem(&clip.source.path);
        let new_path = parent.join(format!(
            "{}_reverse_{}.wav",
            stem,
            uuid::Uuid::new_v4()
        ));

        let spec = hound::WavSpec {
            channels: channels.max(1),
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let write_result = (|| -> Result<()> {
            let mut writer = hound::WavWriter::create(&new_path, spec)?;
            for sample in &reversed {
                writer.write_sample(*sample)?;
            }
            writer.finalize()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            self.error_message = Some(format!("Failed to write reversed audio: {}", e));
            return;
        }

        let new_source = AudioFileRef::new(
            new_path.clone(),
            frame_count as u64,
            sample_rate,
            channels.max(1),
            BitDepth::Float32,
        );
        let from = ClipSourceState::of(clip);
        let to = ClipSourceState {
            source: new_source,
            source_offset: 0,
            length: frame_count as u64,
            fade_in: from.fade_in,
            fade_out: from.fade_out,
            gain: from.gain,
        };
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            to.apply(clip);
        }
        self.push_undo(EditAction::Source { clip_id, from, to }, false);
        self.after_edit();
        self.refresh_playback_if_playing();
        // The rendered file has no cached waveform yet.
        self.spawn_overview(clip_id, new_path);
    }

    /// Bake the clip's gain and fade curves into a new WAV, then reset the
    /// non-destructive gain/fade settings so they are not applied twice.
    fn bake_into_clip(&mut self, clip_id: ClipId) -> bool {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return false;
        };
        if clip.locked || clip.length == 0 {
            return false;
        }
        if (clip.gain - 1.0).abs() < 1e-4
            && clip.fade_in.is_none()
            && clip.fade_out.is_none()
        {
            return false;
        }

        let from = ClipSourceState::of(clip);
        let source_path = clip.source.path.clone();
        let source_offset = clip.source_offset;
        let source_length = clip.length;

        let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
        let render_id = uuid::Uuid::new_v4();
        let output_path = parent.join(format!(
            "{}_process_{}.wav",
            processed_file_stem(&source_path),
            render_id
        ));

        // Match the playback mixer's gain/fade envelope in source frames.
        let clip_frames = source_length;
        let gain = from.gain;
        let fade_in = from.fade_in;
        let fade_out = from.fade_out;
        let envelope = move |frame: u64| {
            clip_gain_at(frame, clip_frames, gain, fade_in, fade_out)
        };

        if let Err(e) = write_wav_range_with_envelope(
            &source_path,
            source_offset,
            source_length,
            &output_path,
            envelope,
        ) {
            let _ = std::fs::remove_file(&output_path);
            self.error_message = Some(format!("Could not render gain and fades: {}", e));
            return false;
        }

        let (frames, spec) = match wav_file_info(&output_path) {
            Ok(info) => info,
            Err(e) => {
                let _ = std::fs::remove_file(&output_path);
                self.error_message =
                    Some(format!("Could not read rendered audio: {}", e));
                return false;
            }
        };
        if frames == 0 {
            let _ = std::fs::remove_file(&output_path);
            self.error_message = Some("Rendered audio is empty.".to_string());
            return false;
        }

        let new_source = AudioFileRef::new(
            output_path.clone(),
            frames,
            spec.sample_rate,
            spec.channels,
            bit_depth_from_spec(spec),
        );
        let to = ClipSourceState {
            source: new_source,
            source_offset: 0,
            length: frames,
            fade_in: None,
            fade_out: None,
            gain: 1.0,
        };
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            to.apply(clip);
        }
        self.push_undo(EditAction::Source { clip_id, from, to }, false);
        self.error_message = None;
        self.after_edit();
        self.refresh_playback_if_playing();
        self.spawn_overview(clip_id, output_path);
        true
    }

    /// Render a pitch shift and/or time stretch to a new WAV and point the clip
    /// at it. The original file is untouched and the edit is undoable.
    fn apply_pitch_stretch(
        &mut self,
        clip_id: ClipId,
        semitones: f32,
        stretch: f32,
        preserve_formants: bool,
        mode: TimeStretchMode,
    ) -> bool {
        let semitones = semitones.clamp(-24.0, 24.0);
        let stretch = stretch.clamp(0.25, 4.0);
        let pitch_active = semitones.abs() >= 0.05;
        let stretch_active = (stretch - 1.0).abs() >= 0.005;
        if !pitch_active && !stretch_active {
            return false;
        }

        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return false;
        };
        if clip.locked || clip.length == 0 {
            return false;
        }

        let from = ClipSourceState::of(clip);
        let source_path = clip.source.path.clone();
        let source_offset = clip.source_offset;
        let source_length = clip.length;

        let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
        let render_id = uuid::Uuid::new_v4();
        let output_path = parent.join(format!(
            "{}_process_{}.wav",
            processed_file_stem(&source_path),
            render_id
        ));

        if mode == TimeStretchMode::Tape && !pitch_active {
            let (samples, channels, sample_rate) =
                match load_wav_range_interleaved(&source_path, source_offset, source_length) {
                    Ok(range) => range,
                    Err(e) => {
                        self.error_message = Some(format!("Could not read clip audio: {}", e));
                        return false;
                    }
                };
            let stretched = tape_stretch_interleaved(&samples, channels, stretch);
            if let Err(e) =
                write_wav_f32(&output_path, &stretched, channels.max(1), sample_rate)
            {
                let _ = std::fs::remove_file(&output_path);
                self.error_message = Some(format!("Could not write stretched audio: {}", e));
                return false;
            }
        } else {
            let temp_path = parent.join(format!(".zoft_process_{}.wav", render_id));
            if let Err(e) =
                write_wav_range(&source_path, source_offset, source_length, &temp_path)
            {
                let _ = std::fs::remove_file(&temp_path);
                self.error_message = Some(format!("Could not prepare clip audio: {}", e));
                return false;
            }

            let render = render_pitch_stretch(
                &temp_path,
                &output_path,
                semitones,
                stretch,
                preserve_formants,
                mode,
            );
            let _ = std::fs::remove_file(&temp_path);
            if let Err(e) = render {
                let _ = std::fs::remove_file(&output_path);
                self.error_message = Some(format!(
                    "Pitch/time processing failed: {}. Is rubberband installed?",
                    e
                ));
                return false;
            }
        }

        let (frames, spec) = match wav_file_info(&output_path) {
            Ok(info) => info,
            Err(e) => {
                let _ = std::fs::remove_file(&output_path);
                self.error_message = Some(format!("Could not read processed audio: {}", e));
                return false;
            }
        };
        if frames == 0 {
            let _ = std::fs::remove_file(&output_path);
            self.error_message = Some("Processed audio is empty.".to_string());
            return false;
        }

        // Keep fade lengths within the rendered clip, which may be shorter
        // after a 0.5x–1.0x stretch.
        let mut fade_in = from.fade_in;
        if let Some(fade) = &mut fade_in {
            fade.length = fade.length.min(frames);
        }
        let mut fade_out = from.fade_out;
        if let Some(fade) = &mut fade_out {
            fade.length = fade.length.min(frames);
        }

        let new_source = AudioFileRef::new(
            output_path.clone(),
            frames,
            spec.sample_rate,
            spec.channels,
            bit_depth_from_spec(spec),
        );
        let to = ClipSourceState {
            source: new_source,
            source_offset: 0,
            length: frames,
            fade_in,
            fade_out,
            gain: from.gain,
        };
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            to.apply(clip);
        }
        self.push_undo(EditAction::Source { clip_id, from, to }, false);
        self.error_message = None;
        self.after_edit();
        self.refresh_playback_if_playing();
        self.spawn_overview(clip_id, output_path);
        true
    }

    /// Merge the selected clip's contiguous chain on its track into one file.
    /// Gain, fades and automatic crossfades are rendered into the new clip;
    /// the original clips remain in the project and are restored by undo.
    fn consolidate_clip(&mut self, clip_id: ClipId) {
        let Some(plan) = self.consolidation_plan(clip_id) else {
            self.error_message = Some("The clip is no longer on a track.".to_string());
            return;
        };
        if plan.clip_ids.len() < 2 {
            self.error_message =
                Some("This clip does not touch another clip on its track.".to_string());
            return;
        }

        let project_sr = self.project.sample_rate.max(1);
        let bpm = self.project.tempo_map.default_bpm();
        let color = self
            .project
            .audio_clips
            .get(clip_id)
            .and_then(|clip| clip.color);
        let mut playable = Vec::new();
        let mut base_path = None;
        let mut out_channels = 1u16;
        let mut any_unmuted = false;

        for id in &plan.clip_ids {
            let Some(clip) = self.project.audio_clips.get(*id) else {
                continue;
            };
            if clip.locked {
                self.error_message =
                    Some("Unlock every clip in the chain before consolidating.".to_string());
                return;
            }
            if base_path.is_none() {
                base_path = Some(clip.source.path.clone());
            }
            if clip.muted || clip.length == 0 {
                continue;
            }

            let (samples, channels, sample_rate) = match load_wav_range_interleaved(
                &clip.source.path,
                clip.source_offset,
                clip.length,
            ) {
                Ok(range) => range,
                Err(e) => {
                    self.error_message =
                        Some(format!("Could not read clip audio: {}", e));
                    return;
                }
            };
            any_unmuted = true;
            out_channels = out_channels.max(channels.max(1));
            playable.push(PlayableClip {
                samples,
                channels,
                sample_rate,
                start_frame: clip.timeline_position.sample.saturating_sub(plan.start),
                track: 0,
                gain: clip.gain,
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
            });
        }

        if !any_unmuted {
            self.error_message =
                Some("All clips in the chain are muted; nothing to consolidate.".to_string());
            return;
        }
        let frames = plan.end.saturating_sub(plan.start);
        if frames == 0 {
            self.error_message = Some("The clips have no audio to consolidate.".to_string());
            return;
        }
        let Some(base_path) = base_path else {
            self.error_message = Some("Could not find the chain's source files.".to_string());
            return;
        };
        let out_channels = out_channels.clamp(1, 2);
        let parent = base_path.parent().unwrap_or_else(|| Path::new(""));
        let output_path = parent.join(format!(
            "{}_process_{}.wav",
            processed_file_stem(&base_path),
            uuid::Uuid::new_v4()
        ));

        let mut mixer = PlaybackMixer::new(project_sr, out_channels, playable, 0);
        let render_result: Result<()> = (|| {
            let spec = hound::WavSpec {
                channels: out_channels,
                sample_rate: project_sr,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer = hound::WavWriter::create(&output_path, spec)?;
            let mut remaining = frames;
            let mut buffer = vec![0.0f32; 8_192 * out_channels as usize];
            while remaining > 0 {
                let chunk = remaining.min(8_192) as usize;
                let samples = chunk * out_channels as usize;
                mixer.fill_unclamped(&mut buffer[..samples]);
                for sample in &buffer[..samples] {
                    writer.write_sample(*sample)?;
                }
                remaining -= chunk as u64;
            }
            writer.finalize()?;
            Ok(())
        })();
        if let Err(e) = render_result {
            let _ = std::fs::remove_file(&output_path);
            self.error_message = Some(format!("Could not write consolidated audio: {}", e));
            return;
        }

        let new_source = AudioFileRef::new(
            output_path.clone(),
            frames,
            project_sr,
            out_channels,
            BitDepth::Float32,
        );
        let mut new_clip = AudioClip::new(
            new_source,
            TimelinePos::from_sample(plan.start, project_sr as f64, bpm),
            frames,
        );
        new_clip.color = color;
        let new_id = self.project.audio_clips.insert(new_clip);

        let mut actions: Vec<EditAction> = plan
            .clip_ids
            .iter()
            .map(|id| EditAction::Detach {
                track: plan.track,
                clip_id: *id,
            })
            .collect();
        actions.push(EditAction::Attach {
            track: plan.track,
            clip_id: new_id,
        });
        let action = EditAction::Group {
            actions,
            label: "Consolidate clips".to_string(),
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.set_single_selection(new_id);
        self.error_message = None;
        self.after_edit();
        self.refresh_playback_if_playing();
        self.spawn_overview(new_id, output_path);
    }

    /// Apply an offline effect to the clip's visible source range and point the
    /// clip at a new rendered WAV. Gain, fades and placement are preserved.
    fn render_clip_effect<F>(&mut self, clip_id: ClipId, mut process: F) -> bool
    where
        F: FnMut(&mut [f32], u16, u32),
    {
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return false;
        };
        if clip.locked || clip.length == 0 {
            return false;
        }

        let from = ClipSourceState::of(clip);
        let source_path = clip.source.path.clone();
        let source_offset = clip.source_offset;
        let source_length = clip.length;

        let (mut samples, channels, sample_rate) = match load_wav_range_interleaved(
            &source_path,
            source_offset,
            source_length,
        ) {
            Ok(range) => range,
            Err(e) => {
                self.error_message =
                    Some(format!("Could not read clip audio: {}", e));
                return false;
            }
        };
        if samples.is_empty() || channels == 0 {
            self.error_message = Some("The clip has no audio to process.".to_string());
            return false;
        }

        process(&mut samples, channels, sample_rate);
        let channel_count = channels as usize;
        let frames = samples.len() as u64 / channel_count as u64;
        if frames == 0 {
            self.error_message = Some("The processed audio is empty.".to_string());
            return false;
        }
        samples.truncate(frames as usize * channel_count);

        let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
        let output_path = parent.join(format!(
            "{}_process_{}.wav",
            processed_file_stem(&source_path),
            uuid::Uuid::new_v4()
        ));
        if let Err(e) = write_wav_f32(&output_path, &samples, channels, sample_rate) {
            let _ = std::fs::remove_file(&output_path);
            self.error_message = Some(format!("Could not write processed audio: {}", e));
            return false;
        }

        let new_source = AudioFileRef::new(
            output_path.clone(),
            frames,
            sample_rate,
            channels,
            BitDepth::Float32,
        );
        let to = ClipSourceState {
            source: new_source,
            source_offset: 0,
            length: frames,
            fade_in: from.fade_in,
            fade_out: from.fade_out,
            gain: from.gain,
        };
        if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
            to.apply(clip);
        }
        self.push_undo(EditAction::Source { clip_id, from, to }, false);
        self.error_message = None;
        self.after_edit();
        self.refresh_playback_if_playing();
        self.spawn_overview(clip_id, output_path);
        true
    }

    fn copy_selected_clip(&mut self) {
        let audio_ids = self.selection_ids();
        if audio_ids.is_empty() {
            self.clipboard.clear();
        } else {
            let mut clips: Vec<AudioClip> = audio_ids
                .into_iter()
                .filter_map(|id| self.project.audio_clips.get(id).cloned())
                .filter(|clip| !clip.locked)
                .collect();
            // Keep a stable left-to-right order so paste preserves the layout.
            clips.sort_by_key(|clip| clip.timeline_position.sample);
            self.clipboard = clips;
        }

        let midi_ids = self.midi_selection_ids();
        if midi_ids.is_empty() {
            self.midi_clipboard.clear();
        } else {
            let mut clips: Vec<MidiClip> = midi_ids
                .into_iter()
                .filter_map(|id| self.project.midi_clips.get(id).cloned())
                .collect();
            clips.sort_by_key(|clip| clip.timeline_position.tick);
            self.midi_clipboard = clips;
        }
    }

    fn paste_clip(&mut self) {
        let audio_track = self
            .selected_clip
            .and_then(|id| self.clip_track(id))
            .or_else(|| self.project.track_order.first().copied());
        if !self.midi_clipboard.is_empty() {
            self.paste_midi_clips();
        }
        if self.clipboard.is_empty() {
            return;
        }
        let Some(track_id) = audio_track else {
            self.error_message = Some("No track to paste into.".to_string());
            return;
        };

        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        // Paste the group with its earliest clip at the playhead.
        let earliest = self
            .clipboard
            .iter()
            .map(|clip| clip.timeline_position.sample)
            .min()
            .unwrap_or(0);
        let target_start = (self.playhead_secs.max(0.0) * project_sr).round() as u64;
        let delta = target_start.saturating_sub(earliest);

        let mut new_ids = Vec::new();
        self.begin_undo_group("Paste clips");
        for template in self.clipboard.clone() {
            let mut clip = template;
            clip.id = ClipId::default();
            clip.timeline_position = TimelinePos::from_sample(
                clip.timeline_position.sample.saturating_add(delta),
                project_sr,
                bpm,
            );
            let new_id = self.project.audio_clips.insert(clip);
            let action = EditAction::Attach {
                track: track_id,
                clip_id: new_id,
            };
            self.apply_action(&action, true);
            self.push_undo(action, false);
            new_ids.push(new_id);
        }
        self.end_undo_group();

        if !new_ids.is_empty() {
            self.selected_clips = new_ids.iter().copied().collect();
            self.selected_clip = new_ids.last().copied();
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    /// Paste copied MIDI clips at the playhead on the selected MIDI track.
    fn paste_midi_clips(&mut self) {
        let track_id = self
            .selected_midi_clip
            .and_then(|id| self.midi_clip_track(id))
            .or_else(|| {
                self.project.track_order.iter().copied().find(|id| {
                    self.project
                        .tracks
                        .get(id)
                        .map(|track| {
                            matches!(
                                track.track_type,
                                TrackType::Midi | TrackType::Instrument
                            )
                        })
                        .unwrap_or(false)
                })
            });
        let Some(track_id) = track_id else {
            self.error_message =
                Some("No MIDI track to paste into.".to_string());
            return;
        };

        let project_sr = self.project.sample_rate.max(1) as f64;
        let target_sample =
            (self.playhead_secs.max(0.0) * project_sr).round() as u64;
        let target_tick = self
            .project
            .tempo_map
            .sample_to_tick(target_sample, project_sr);
        let earliest = self
            .midi_clipboard
            .iter()
            .map(|clip| clip.timeline_position.tick)
            .min()
            .unwrap_or(0);
        let delta = target_tick.saturating_sub(earliest);

        let mut new_ids = Vec::new();
        self.begin_undo_group("Paste MIDI clips");
        for template in self.midi_clipboard.clone() {
            let mut clip = template;
            clip.id = ClipId::default();
            let new_tick = clip.timeline_position.tick.saturating_add(delta);
            clip.timeline_position.tick = new_tick;
            clip.timeline_position.sample =
                self.project.tempo_map.tick_to_sample(new_tick, project_sr);
            let new_id = self.project.midi_clips.insert(clip);
            let action = EditAction::AttachMidi {
                track: track_id,
                clip_id: new_id,
            };
            self.apply_action(&action, true);
            self.push_undo(action, false);
            new_ids.push(new_id);
        }
        self.end_undo_group();

        if !new_ids.is_empty() {
            self.clear_selection();
            for clip_id in &new_ids {
                self.selected_midi_clips.insert(*clip_id);
            }
            self.selected_midi_clip = new_ids.last().copied();
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    fn duplicate_selected_clip(&mut self) {
        let midi_ids = self.midi_selection_ids();
        let ids = self.selection_ids();
        if midi_ids.is_empty() && ids.is_empty() {
            return;
        }
        if !midi_ids.is_empty() {
            self.duplicate_midi_clips(midi_ids);
        }
        if ids.is_empty() {
            return;
        }
        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();

        // Duplicate the selection as a group, shifted by its total span.
        let mut span_start = f64::MAX;
        let mut span_end = f64::MIN;
        for clip_id in &ids {
            let Some(clip) = self.project.audio_clips.get(*clip_id) else {
                continue;
            };
            let start = clip.timeline_position.sample as f64 / project_sr;
            let end = start + clip.length as f64 / clip.source.sample_rate.max(1) as f64;
            span_start = span_start.min(start);
            span_end = span_end.max(end);
        }
        if !span_start.is_finite() || !span_end.is_finite() {
            return;
        }
        let offset_secs = (span_end - span_start).max(MIN_CLIP_SECS);

        let mut new_ids = Vec::new();
        self.begin_undo_group("Duplicate clips");
        for clip_id in ids {
            let Some(track_id) = self.clip_track(clip_id) else {
                continue;
            };
            let Some(clip) = self.project.audio_clips.get(clip_id) else {
                continue;
            };
            if clip.locked {
                continue;
            }

            let mut copy = clip.clone();
            copy.id = ClipId::default();
            copy.timeline_position = TimelinePos::from_sample(
                copy.timeline_position.sample
                    + (offset_secs * project_sr).round() as u64,
                project_sr,
                bpm,
            );

            let new_id = self.project.audio_clips.insert(copy);
            let action = EditAction::Attach {
                track: track_id,
                clip_id: new_id,
            };
            self.apply_action(&action, true);
            self.push_undo(action, false);
            new_ids.push(new_id);
        }
        self.end_undo_group();

        if !new_ids.is_empty() {
            self.selected_clips = new_ids.iter().copied().collect();
            self.selected_clip = new_ids.last().copied();
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    /// Duplicate a MIDI clip immediately after its visible span.
    fn duplicate_midi_clips(&mut self, ids: Vec<ClipId>) {
        if ids.is_empty() {
            return;
        }
        let mut span_start = u64::MAX;
        let mut span_end = 0u64;
        for clip_id in &ids {
            let Some(clip) = self.project.midi_clips.get(*clip_id) else {
                continue;
            };
            span_start = span_start.min(clip.timeline_position.tick);
            span_end = span_end.max(
                clip.timeline_position
                    .tick
                    .saturating_add(clip.length.max(1)),
            );
        }
        if span_start == u64::MAX {
            return;
        }
        let offset = span_end.saturating_sub(span_start).max(1);
        let project_sr = self.project.sample_rate.max(1) as f64;

        let mut new_ids = Vec::new();
        self.begin_undo_group("Duplicate MIDI clips");
        for clip_id in ids {
            let Some(track_id) = self.midi_clip_track(clip_id) else {
                continue;
            };
            let Some(clip) = self.project.midi_clips.get(clip_id).cloned() else {
                continue;
            };
            let new_tick = clip.timeline_position.tick.saturating_add(offset);
            let mut copy = clip;
            copy.id = ClipId::default();
            copy.timeline_position.tick = new_tick;
            copy.timeline_position.sample =
                self.project.tempo_map.tick_to_sample(new_tick, project_sr);
            let new_id = self.project.midi_clips.insert(copy);
            let action = EditAction::AttachMidi {
                track: track_id,
                clip_id: new_id,
            };
            self.apply_action(&action, true);
            self.push_undo(action, false);
            new_ids.push(new_id);
        }
        self.end_undo_group();

        if !new_ids.is_empty() {
            self.clear_selection();
            for clip_id in &new_ids {
                self.selected_midi_clips.insert(*clip_id);
            }
            self.selected_midi_clip = new_ids.last().copied();
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    /// Clip gain, fade and colour controls shown in the Clip Audio window.
    /// Every change is recorded for undo; processing lives in the same
    /// window's effect list.
    fn clip_properties_ui(&mut self, ui: &mut egui::Ui) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };

        let before = ClipShape::of(clip);
        let mut after = before;
        let name = clip
            .source
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("clip")
            .to_string();
        let src_sr = clip.source.sample_rate.max(1) as f64;

        ui.label(egui::RichText::new("Clip").strong());
        ui.label(name);
        ui.weak("Gain, fades and colour apply immediately and are undoable.");
        ui.add_space(4.0);

        // Gain (stored linearly, edited in dB).
        ui.label("Gain:");
        let mut gain_db = 20.0 * before.gain.max(1e-6).log10();
        let gain_response = add_wheel_slider(
            ui,
            &mut gain_db,
            -48.0..=12.0,
            |slider| slider.suffix(" dB").max_decimals(1),
        );
        if gain_response.changed() {
            after.gain = 10.0f32.powf(gain_db / 20.0);
        }
        ui.separator();

        // Fades: curve + length, editable numerically as well as by dragging
        // the handles in the timeline.
        let (fade_in, in_changed) =
            fade_editor_ui(ui, "Fade in", before.fade_in, before.length, src_sr);
        let (fade_out, out_changed) =
            fade_editor_ui(ui, "Fade out", before.fade_out, before.length, src_sr);
        after.fade_in = fade_in;
        after.fade_out = fade_out;

        ui.separator();

        // Per-clip color override.
        ui.label("Color:");
        let mut tint = before
            .color
            .map(|color| {
                egui::Color32::from_rgba_unmultiplied(color.0, color.1, color.2, color.3)
            })
            .unwrap_or(egui::Color32::from_gray(140));
        if ui.color_edit_button_srgba(&mut tint).changed() {
            after.color = Some(Color::from_rgba(tint.r(), tint.g(), tint.b(), tint.a()));
        }
        if before.color.is_some() && ui.small_button("×").clicked() {
            after.color = None;
        }

        ui.separator();
        let mut muted = before.muted;
        if ui.checkbox(&mut muted, "Mute take").changed() {
            after.muted = muted;
        }

        if after != before {
            if self.selected_clips.len() > 1 {
                // Multi-selection: apply the edited fields to every clip and
                // commit one grouped undo entry when the pointer is released.
                if self.pending_shape_edit.is_none() {
                    let label = if after.gain != before.gain {
                        "Clip gain"
                    } else if after.fade_in != before.fade_in || after.fade_out != before.fade_out {
                        "Clip fades"
                    } else if after.color != before.color {
                        "Clip colour"
                    } else {
                        "Clip edit"
                    }
                    .to_string();
                    let snapshots: Vec<(ClipId, ClipShape)> = self
                        .selection_ids()
                        .into_iter()
                        .filter_map(|id| {
                            self.project
                                .audio_clips
                                .get(id)
                                .map(|clip| (id, ClipShape::of(clip)))
                        })
                        .collect();
                    self.pending_shape_edit = Some((label, snapshots));
                }
                for id in self.selection_ids() {
                    let Some(clip) = self.project.audio_clips.get_mut(id) else {
                        continue;
                    };
                    if after.gain != before.gain {
                        clip.gain = after.gain;
                    }
                    if after.fade_in != before.fade_in {
                        clip.fade_in = clamp_fade(after.fade_in, clip.length);
                    }
                    if after.fade_out != before.fade_out {
                        clip.fade_out = clamp_fade(after.fade_out, clip.length);
                    }
                    if after.color != before.color {
                        clip.color = after.color;
                    }
                    if after.muted != before.muted {
                        clip.muted = after.muted;
                    }
                }
                self.after_edit();
                self.needs_audio_refresh = true;
            } else {
                if let Some(clip) = self.project.audio_clips.get_mut(clip_id) {
                    after.apply(clip);
                }
                self.push_undo(
                    EditAction::Shape {
                        clip_id,
                        from: before,
                        to: after,
                    },
                    true,
                );
                self.after_edit();
                // Discrete changes (curve/color) should be heard immediately;
                // while a slider is being dragged we wait for release to avoid
                // glitching.
                let using_pointer = ui.ctx().is_using_pointer();
                if !using_pointer || in_changed || out_changed {
                    self.refresh_playback_if_playing();
                } else {
                    self.needs_audio_refresh = true;
                }
            }
        }

        if gain_response.drag_stopped() {
            self.refresh_playback_if_playing();
        }
    }

    /// Floating edit-history panel. Every edit is listed in order; clicking an
    /// entry jumps the project to that point in the timeline.
    fn show_history_window(&mut self, ctx: &egui::Context) {
        if !self.history_open {
            return;
        }

        let past: Vec<(usize, String)> = self
            .history
            .past()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index + 1, entry.description.clone()))
            .collect();
        let future: Vec<(usize, String)> = self
            .history
            .future()
            .iter()
            .enumerate()
            .map(|(index, entry)| (self.history.past_len() + index + 1, entry.description.clone()))
            .collect();
        let current = self.history.past_len();
        let mut jump = None;
        let mut open = self.history_open;

        egui::Window::new("History")
            .open(&mut open)
            .default_width(260.0)
            .max_width(420.0)
            .show(ctx, |ui| {
                ui.weak("Click an edit to jump the project to that point.");
                ui.add_space(4.0);
                if past.is_empty() && future.is_empty() {
                    ui.label("No edits yet.");
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        // Newest at the top: redoable future states first (most
                        // distant first), then the current point, then past
                        // edits in reverse chronological order.
                        for (index, description) in future.iter().rev() {
                            if ui
                                .selectable_label(
                                    false,
                                    format!("{}. {} (redo)", index, description),
                                )
                                .clicked()
                            {
                                jump = Some(*index);
                            }
                        }
                        ui.separator();
                        ui.weak("current state");
                        for (index, description) in past.iter().rev() {
                            if ui
                                .selectable_label(
                                    *index == current,
                                    format!("{}. {}", index, description),
                                )
                                .clicked()
                            {
                                jump = Some(*index);
                            }
                        }
                    });
            });

        self.history_open = open;
        if let Some(target) = jump {
            if target != self.history.past_len() {
                self.jump_history_to(target);
            }
        }
    }

    /// Floating clip-audio window: gain, fades and colour plus the offline
    /// tools that render into the clip's audio. Opened from the Arrange
    /// **Audio…** button; real-time effects live on tracks instead.
    fn show_audio_window(&mut self, ctx: &egui::Context) {
        let Some(mut window) = self.audio_window.clone() else {
            return;
        };
        // Keep the window in step with the current clip selection.
        let live_ids = self.selection_ids();
        if !live_ids.is_empty() {
            window.clip_id = live_ids[0];
            window.clip_ids = live_ids;
        }
        let clip = self.project.audio_clips.get(window.clip_id);
        let clip_present = clip.is_some();
        let clip_name = clip
            .map(|clip| {
                clip.source
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("clip")
                    .to_string()
            })
            .unwrap_or_else(|| "(clip removed)".to_string());
        let clip_seconds = clip.map(|clip| {
            clip.length as f64 / clip.source.sample_rate.max(1) as f64
        });
        let clip_gain_db = 20.0
            * clip
                .map(|clip| clip.gain.max(1e-6))
                .unwrap_or(1.0)
                .log10();
        let clip_sr = clip
            .map(|clip| clip.source.sample_rate.max(1) as f64)
            .unwrap_or(1.0);
        let clip_fade_in_secs = clip
            .and_then(|clip| clip.fade_in)
            .map(|fade| fade.length as f64 / clip_sr)
            .unwrap_or(0.0);
        let clip_fade_out_secs = clip
            .and_then(|clip| clip.fade_out)
            .map(|fade| fade.length as f64 / clip_sr)
            .unwrap_or(0.0);
        let has_gain_fades = window.clip_ids.iter().any(|id| {
            self.project
                .audio_clips
                .get(*id)
                .map_or(false, |clip| {
                    (clip.gain - 1.0).abs() > 1e-4
                        || clip.fade_in.is_some()
                        || clip.fade_out.is_some()
                })
        });
        let consolidation = self.consolidation_plan(window.clip_id);
        let consolidation_count = consolidation
            .as_ref()
            .map_or(0, |plan| plan.clip_ids.len());
        let consolidation_seconds = consolidation.as_ref().map(|plan| {
            plan.end.saturating_sub(plan.start) as f64
                / self.project.sample_rate.max(1) as f64
        });
        let consolidation_locked = consolidation.as_ref().map_or(false, |plan| {
            plan.clip_ids.iter().any(|id| {
                self.project
                    .audio_clips
                    .get(*id)
                    .map_or(false, |clip| clip.locked)
            })
        });
        let mut open = true;
        let mut apply = false;

        egui::Window::new("Clip Audio")
            .id(egui::Id::new("clip_audio_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(430.0)
            .max_width(460.0)
            .show(ctx, |ui| {
                ui.set_max_width(400.0);
                let title = if window.clip_ids.len() > 1 {
                    format!("{} clips", window.clip_ids.len())
                } else {
                    clip_name.clone()
                };
                ui.horizontal(|ui| {
                    ui.label("Clip:");
                    ui.label(egui::RichText::new(title).strong());
                });
                if let Some(seconds) = clip_seconds {
                    ui.weak(format!("Length: {:.2} s", seconds));
                }
                ui.separator();

                if !clip_present {
                    ui.label("This clip is no longer in the project.");
                    return;
                }

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(120.0);
                        ui.label(
                            egui::RichText::new("CLIP AUDIO")
                                .small()
                                .strong(),
                        );
                        ui.add_space(3.0);
                        // A fixed-width rule keeps the header visually distinct
                        // without making the window expand to the available width.
                        let (rule, _) = ui.allocate_exact_size(
                            egui::vec2(115.0, 1.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().hline(
                            rule.x_range(),
                            rule.center().y,
                            ui.visuals().widgets.noninteractive.bg_stroke,
                        );
                        ui.add_space(4.0);
                        if ui
                            .selectable_label(window.selected.is_none(), "Clip")
                            .on_hover_text("Gain, fades and colour")
                            .clicked()
                        {
                            window.selected = None;
                        }
                        for effect in EffectKind::ALL {
                            if ui
                                .selectable_label(window.selected == Some(effect), effect.label())
                                .clicked()
                            {
                                window.selected = Some(effect);
                            }
                        }
                    });

                    ui.separator();

                    ui.vertical(|ui| {
                        ui.set_min_width(230.0);
                        match window.selected {
                            None => {
                                self.clip_properties_ui(ui);
                            }
                            Some(effect) => match effect {
                                EffectKind::Normalize => {
                                ui.label(egui::RichText::new("Normalize").strong());
                                ui.weak(
                                    "Set the clip gain so its loudest sample peaks at −1 dBFS.",
                                );
                            }
                            EffectKind::LufsNormalize => {
                                ui.label(egui::RichText::new("LUFS Normalize").strong());
                                knob(
                                    ui,
                                    &mut window.lufs_target,
                                    -30.0..=-6.0,
                                    -14.0,
                                    "Target",
                                    false,
                                    |value| format!("{:.1} LUFS", value),
                                );
                                ui.weak(
                                    "Measures integrated loudness over the clip and applies \
                                     gain to hit the target. Gain is capped so sample peaks \
                                     stay at or below −1 dBFS.",
                                );
                            }
                            EffectKind::BakeIntoClip => {
                                ui.label(egui::RichText::new("Bake into Clip").strong());
                                ui.label(format!("Gain: {:+.1} dB", clip_gain_db));
                                ui.label(format!("Fade in: {:.2} s", clip_fade_in_secs));
                                ui.label(format!("Fade out: {:.2} s", clip_fade_out_secs));
                                ui.weak(
                                    "Bakes the clip's gain and fades into a new audio file, then \
                                     resets the gain to 0 dB and removes the fades.",
                                );
                            }
                            EffectKind::Reverse => {
                                ui.label(egui::RichText::new("Reverse").strong());
                                ui.weak(
                                    "Render the clip's audible range backwards to a new file. \
                                     The original audio is kept.",
                                );
                            }
                            EffectKind::PitchShift => {
                                ui.label(egui::RichText::new("Pitch Shift").strong());
                                knob(
                                    ui,
                                    &mut window.pitch,
                                    -12.0..=12.0,
                                    0.0,
                                    "Pitch",
                                    false,
                                    |value| format!("{:+.2} st", value),
                                );
                                ui.checkbox(&mut window.formants, "Preserve formants");
                                ui.weak("Positive values raise the pitch; 0.01 st = 1 cent.");
                            }
                            EffectKind::TimeStretch => {
                                ui.label(egui::RichText::new("Time Stretch").strong());
                                ui.horizontal(|ui| {
                                    ui.label("Mode:");
                                    egui::ComboBox::from_id_salt("effect_stretch_mode")
                                        .selected_text(stretch_mode_label(window.stretch_mode))
                                        .width(110.0)
                                        .show_ui(ui, |ui| {
                                            for mode in [
                                                TimeStretchMode::Tape,
                                                TimeStretchMode::Elastic,
                                                TimeStretchMode::Musical,
                                            ] {
                                                ui.selectable_value(
                                                    &mut window.stretch_mode,
                                                    mode,
                                                    stretch_mode_label(mode),
                                                );
                                            }
                                        });
                                });
                                knob(
                                    ui,
                                    &mut window.stretch,
                                    0.5..=2.0,
                                    1.0,
                                    "Stretch",
                                    false,
                                    |value| format!("{:.2}×", value),
                                );
                                let target = clip_seconds.map(|seconds| seconds * window.stretch as f64);
                                if let Some(seconds) = target {
                                    ui.weak(format!("New clip length: {:.2} s", seconds));
                                }
                                ui.weak(match window.stretch_mode {
                                    TimeStretchMode::Tape => {
                                        "Varispeed: length and pitch change together, like tape."
                                    }
                                    TimeStretchMode::Elastic => {
                                        "Rubber Band R3 fine mode; smooth and best for sustained sounds."
                                    }
                                    TimeStretchMode::Musical => {
                                        "Rubber Band R3 with centre focus; best for mixed material."
                                    }
                                });
                            }
                            EffectKind::BitCrusher => {
                                ui.label(egui::RichText::new("Bit Crusher").strong());
                                ui.horizontal_top(|ui| {
                                    knob(
                                        ui,
                                        &mut window.crush_bits,
                                        1.0..=16.0,
                                        8.0,
                                        "Bits",
                                        false,
                                        |value| format!("{:.0} bit", value),
                                    );
                                    knob(
                                        ui,
                                        &mut window.crush_downsample,
                                        1.0..=64.0,
                                        4.0,
                                        "Hold",
                                        false,
                                        |value| format!("{:.0}×", value),
                                    );
                                });
                                ui.weak(
                                    "Reduces the bit depth and holds samples for a crunchy, \
                                     digital character.",
                                );
                            }
                            EffectKind::Saturation => {
                                ui.label(egui::RichText::new("Saturation").strong());
                                ui.horizontal_top(|ui| {
                                    knob(
                                        ui,
                                        &mut window.drive_db,
                                        0.0..=36.0,
                                        12.0,
                                        "Drive",
                                        false,
                                        |value| format!("{:.1} dB", value),
                                    );
                                    knob(
                                        ui,
                                        &mut window.saturation_mix,
                                        0.0..=100.0,
                                        100.0,
                                        "Mix",
                                        false,
                                        |value| format!("{:.0}%", value),
                                    );
                                });
                                ui.weak(
                                    "Soft clipping adds harmonics. Mix blends the saturated \
                                     signal with the dry audio.",
                                );
                            }
                            EffectKind::Filter => {
                                ui.label(egui::RichText::new("Filter").strong());
                                ui.horizontal(|ui| {
                                    ui.label("Type:");
                                    egui::ComboBox::from_id_salt("effect_filter_kind")
                                        .selected_text(window.filter_kind.label())
                                        .width(110.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut window.filter_kind,
                                                FilterKind::LowPass,
                                                FilterKind::LowPass.label(),
                                            );
                                            ui.selectable_value(
                                                &mut window.filter_kind,
                                                FilterKind::HighPass,
                                                FilterKind::HighPass.label(),
                                            );
                                        });
                                });
                                knob(
                                    ui,
                                    &mut window.filter_cutoff,
                                    20.0..=20_000.0,
                                    8_000.0,
                                    "Cutoff",
                                    true,
                                    format_frequency,
                                );
                                ui.weak(
                                    "First-order filter. Cutoff is clamped below Nyquist on \
                                     render.",
                                );
                            }
                            EffectKind::Tremolo => {
                                ui.label(egui::RichText::new("Tremolo").strong());
                                ui.horizontal_top(|ui| {
                                    knob(
                                        ui,
                                        &mut window.tremolo_rate,
                                        0.1..=20.0,
                                        5.0,
                                        "Rate",
                                        false,
                                        |value| format!("{:.2} Hz", value),
                                    );
                                    knob(
                                        ui,
                                        &mut window.tremolo_depth,
                                        0.0..=100.0,
                                        50.0,
                                        "Depth",
                                        false,
                                        |value| format!("{:.0}%", value),
                                    );
                                });
                                ui.weak("Modulates the clip volume at the chosen rate.");
                            }
                            EffectKind::Consolidate => {
                                ui.label(egui::RichText::new("Consolidate").strong());
                                if let Some(seconds) = consolidation_seconds {
                                    ui.label(format!(
                                        "Clips in chain: {}  ·  {:.2} s",
                                        consolidation_count, seconds
                                    ));
                                } else {
                                    ui.weak("This clip is no longer on a track.");
                                }
                                if consolidation_count < 2 {
                                    ui.weak(
                                        "This clip does not touch another clip on its track.",
                                    );
                                } else if consolidation_locked {
                                    ui.weak(
                                        "Unlock every clip in the chain before consolidating.",
                                    );
                                }
                                ui.weak(
                                    "Renders touching or overlapping clips on this track into \
                                     one new file, baking in gain, fades and automatic \
                                     crossfades. The original clips stay in the project and \
                                     undo restores them.",
                                );
                            }
                            },
                        }

                        ui.add_space(10.0);
                        if let Some(effect) = window.selected {
                            if window.clip_ids.len() > 1 {
                                ui.weak(format!(
                                    "Applies to {} selected clips.",
                                    window.clip_ids.len()
                                ));
                                ui.add_space(4.0);
                            }
                            let can_apply = match effect {
                                EffectKind::Normalize
                                | EffectKind::LufsNormalize
                                | EffectKind::Reverse => true,
                                EffectKind::BakeIntoClip => has_gain_fades,
                                EffectKind::PitchShift => window.pitch.abs() >= 0.05,
                                EffectKind::TimeStretch => {
                                    (window.stretch - 1.0).abs() >= 0.005
                                }
                                EffectKind::BitCrusher | EffectKind::Filter => true,
                                EffectKind::Saturation => window.saturation_mix > 0.0,
                                EffectKind::Tremolo => window.tremolo_depth > 0.0,
                                EffectKind::Consolidate => {
                                    window.clip_ids.len() == 1
                                        && consolidation_count >= 2
                                        && !consolidation_locked
                                }
                            };
                            if ui
                                .add_enabled(can_apply, egui::Button::new("Apply"))
                                .clicked()
                            {
                                apply = true;
                            }
                        } else {
                            ui.weak(
                                "Gain, fades and colour apply immediately and are undoable.",
                            );
                        }
                    });
                });
            });

        if !open {
            self.audio_window = None;
            return;
        }

        if apply {
            if let Some(effect) = window.selected {
                let ids = window.clip_ids.clone();
                if !ids.is_empty() {
                    self.begin_undo_group(format!("Apply {}", effect.label()));
                    let mut consolidated = false;
                    match effect {
                        EffectKind::Normalize => {
                            for id in &ids {
                                self.normalize_clip(*id);
                            }
                        }
                        EffectKind::LufsNormalize => {
                            for id in &ids {
                                self.lufs_normalize_clip(*id, window.lufs_target);
                            }
                        }
                        EffectKind::BakeIntoClip => {
                            for id in &ids {
                                self.bake_into_clip(*id);
                            }
                        }
                        EffectKind::Reverse => {
                            for id in &ids {
                                self.reverse_clip(*id);
                            }
                        }
                        EffectKind::PitchShift => {
                            let mut changed = false;
                            for id in &ids {
                                changed |= self.apply_pitch_stretch(
                                    *id,
                                    window.pitch,
                                    1.0,
                                    window.formants,
                                    TimeStretchMode::Elastic,
                                );
                            }
                            if changed {
                                window.pitch = 0.0;
                            }
                        }
                        EffectKind::TimeStretch => {
                            let mut changed = false;
                            for id in &ids {
                                changed |= self.apply_pitch_stretch(
                                    *id,
                                    0.0,
                                    window.stretch,
                                    false,
                                    window.stretch_mode,
                                );
                            }
                            if changed {
                                window.stretch = 1.0;
                            }
                        }
                        EffectKind::BitCrusher => {
                            let bits = window.crush_bits.round() as u32;
                            let downsample = window.crush_downsample.round() as u32;
                            for id in &ids {
                                self.render_clip_effect(
                                    *id,
                                    move |samples, channels, _sample_rate| {
                                        bit_crush(samples, channels, bits, downsample);
                                    },
                                );
                            }
                        }
                        EffectKind::Saturation => {
                            let drive_db = window.drive_db;
                            let mix = window.saturation_mix / 100.0;
                            for id in &ids {
                                self.render_clip_effect(
                                    *id,
                                    move |samples, _channels, _sample_rate| {
                                        saturate(samples, drive_db, mix);
                                    },
                                );
                            }
                        }
                        EffectKind::Filter => {
                            let kind = window.filter_kind;
                            let cutoff = window.filter_cutoff;
                            for id in &ids {
                                self.render_clip_effect(
                                    *id,
                                    move |samples, channels, sample_rate| {
                                        filter_audio(samples, channels, sample_rate, cutoff, kind);
                                    },
                                );
                            }
                        }
                        EffectKind::Tremolo => {
                            let rate = window.tremolo_rate;
                            let depth = window.tremolo_depth / 100.0;
                            for id in &ids {
                                self.render_clip_effect(
                                    *id,
                                    move |samples, channels, sample_rate| {
                                        tremolo(samples, channels, sample_rate, rate, depth);
                                    },
                                );
                            }
                        }
                        EffectKind::Consolidate => {
                            if ids.len() == 1 {
                                let previous = ids[0];
                                self.consolidate_clip(previous);
                                consolidated = true;
                                if self.selected_clip != Some(previous) {
                                    // The clip was replaced by the consolidated result.
                                    self.end_undo_group();
                                    self.audio_window = None;
                                    return;
                                }
                            }
                        }
                    }
                    if !consolidated {
                        self.end_undo_group();
                    }
                }
            }
        }

        self.audio_window = Some(window);
    }

    /// Logic-style insert rack: every track is a horizontal column with its
    /// effect chain underneath.
    fn show_fx_rack(&mut self, ctx: &egui::Context) {
        if !self.fx_rack_open {
            return;
        }
        let track_ids = self.project.track_order.clone();
        let engine_meters = self.engine.track_meters();
        let playing = self.playing;
        for (index, track_id) in track_ids.iter().enumerate() {
            let target = if playing {
                engine_meters
                    .get(index)
                    .map(|meter| meter.peak)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let level = self
                .track_meter_levels
                .entry(*track_id)
                .or_insert(0.0);
            *level = if target >= *level {
                target
            } else {
                (*level - 0.02).max(0.0)
            };
            let hold = self
                .track_meter_holds
                .entry(*track_id)
                .or_insert(0.0);
            *hold = (*hold * 0.995).max(*level);
        }
        let master = self.engine.master_meter();
        let master_targets = if playing {
            [master.left_peak, master.right_peak]
        } else {
            [0.0, 0.0]
        };
        for index in 0..2 {
            self.master_meter_levels[index] = if master_targets[index]
                >= self.master_meter_levels[index]
            {
                master_targets[index]
            } else {
                (self.master_meter_levels[index] - 0.02).max(0.0)
            };
        }
        let mut open = true;
        let mut changes: Vec<(TrackId, Vec<InsertSlot>)> = Vec::new();
        let mut track_changes: Vec<(TrackId, TrackShape, TrackShape, bool)> =
            Vec::new();
        let clipboard = self.insert_clipboard.clone();
        let mut copy_chain = None;
        let column_width = self.fx_rack_column_width.pixels();

        egui::Window::new("FX Rack")
            .id(egui::Id::new("fx_rack"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([1120.0, 640.0])
            .min_size([620.0, 360.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("FX Rack");
                    ui.weak(
                        "Press X to toggle. Inserts run before each track fader.",
                    );
                    ui.separator();
                    ui.label("Columns:");
                    egui::ComboBox::from_id_salt("fx_rack_column_width")
                        .selected_text(self.fx_rack_column_width.label())
                        .width(84.0)
                        .show_ui(ui, |ui| {
                            for width in MixerColumnWidth::ALL {
                                ui.selectable_value(
                                    &mut self.fx_rack_column_width,
                                    width,
                                    width.label(),
                                );
                            }
                        });
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            draw_master_meter(
                                ui,
                                self.master_meter_levels,
                            );
                        },
                    );
                });
                ui.separator();

                if track_ids.is_empty() {
                    ui.weak("No tracks in this project.");
                    return;
                }

                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for track_id in &track_ids {
                                let Some(track) =
                                    self.project.tracks.get(track_id)
                                else {
                                    continue;
                                };
                                let track_name = track.name.clone();
                                let track_type = track.track_type;
                                let color = egui::Color32::from_rgb(
                                    track.color.0,
                                    track.color.1,
                                    track.color.2,
                                );
                                let mut inserts = track.inserts.clone();
                                let before = TrackShape::of(track);
                                let mut after = before;
                                ui.push_id(track_id, |ui| {
                                    ui.vertical(|ui| {
                                        ui.set_width(column_width);
                                        egui::Frame::group(ui.style()).show(
                                            ui,
                                            |ui| {
                                                ui.set_width(
                                                    ui.available_width(),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.colored_label(
                                                        color, "■",
                                                    );
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(
                                                                &track_name,
                                                            )
                                                            .strong(),
                                                        )
                                                        .truncate(),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            let solo_color =
                                                                if after.solo {
                                                                    egui::Color32::GREEN
                                                                } else {
                                                                    egui::Color32::GRAY
                                                                };
                                                            if ui
                                                                .add(
                                                                    egui::Button::new(
                                                                        egui::RichText::new("S")
                                                                            .color(solo_color),
                                                                    )
                                                                    .min_size(
                                                                        [18.0, 18.0].into(),
                                                                    ),
                                                                )
                                                                .on_hover_text("Solo")
                                                                .clicked()
                                                            {
                                                                after.solo = !after.solo;
                                                            }
                                                            let mute_color =
                                                                if after.mute {
                                                                    egui::Color32::YELLOW
                                                                } else {
                                                                    egui::Color32::GRAY
                                                                };
                                                            if ui
                                                                .add(
                                                                    egui::Button::new(
                                                                        egui::RichText::new("M")
                                                                            .color(mute_color),
                                                                    )
                                                                    .min_size(
                                                                        [18.0, 18.0].into(),
                                                                    ),
                                                                )
                                                                .on_hover_text("Mute")
                                                                .clicked()
                                                            {
                                                                after.mute = !after.mute;
                                                            }
                                                        },
                                                    );
                                                });
                                                ui.weak(track_type_label(
                                                    track_type,
                                                ));
                                                ui.horizontal(|ui| {
                                                    ui.label("Vol");
                                                    add_sized_wheel_slider_step(
                                                        ui,
                                                        egui::vec2(120.0, 18.0),
                                                        &mut after.volume,
                                                        0.0..=1.5,
                                                        Some(0.05),
                                                        false,
                                                        |slider| {
                                                            slider.show_value(false)
                                                        },
                                                    )
                                                    .on_hover_text(format!(
                                                        "Track volume {:.0}%",
                                                        after.volume * 100.0
                                                    ));
                                                    ui.label(format!(
                                                        "{:.0}%",
                                                        after.volume * 100.0
                                                    ));
                                                });
                                                ui.horizontal(|ui| {
                                                    ui.label("Pan");
                                                    let mut pan =
                                                        after.pan.to_linear();
                                                    if compact_knob(
                                                        ui,
                                                        &mut pan,
                                                        -1.0..=1.0,
                                                        0.0,
                                                        false,
                                                    )
                                                    .changed()
                                                    {
                                                        after.pan =
                                                            PanLaw::from_linear(
                                                                pan,
                                                            );
                                                    }
                                                    let pan_text =
                                                        if pan.abs() < 0.005 {
                                                            "C".to_string()
                                                        } else if pan < 0.0 {
                                                            format!(
                                                                "L{:.0}",
                                                                -pan * 100.0
                                                            )
                                                        } else {
                                                            format!(
                                                                "R{:.0}",
                                                                pan * 100.0
                                                            )
                                                        };
                                                    ui.label(pan_text);
                                                });
                                                let meter = self
                                                    .track_meter_levels
                                                    .get(track_id)
                                                    .copied()
                                                    .unwrap_or(0.0);
                                                let hold = self
                                                    .track_meter_holds
                                                    .get(track_id)
                                                    .copied()
                                                    .unwrap_or(0.0);
                                                draw_track_meter(
                                                    ui, meter, hold,
                                                );
                                                ui.horizontal(|ui| {
                                                    if ui
                                                        .small_button("Copy FX")
                                                        .on_hover_text(
                                                            "Copy this track's insert chain",
                                                        )
                                                        .clicked()
                                                    {
                                                        copy_chain = Some(
                                                            inserts.clone(),
                                                        );
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            clipboard.is_some(),
                                                            egui::Button::new(
                                                                "Paste FX",
                                                            ),
                                                        )
                                                        .on_hover_text(
                                                            "Replace this track's inserts with the copied chain",
                                                        )
                                                        .clicked()
                                                    {
                                                        if let Some(chain) =
                                                            &clipboard
                                                        {
                                                            changes.push((
                                                                *track_id,
                                                                chain.clone(),
                                                            ));
                                                        }
                                                    }
                                                });
                                                ui.separator();
                                                if show_insert_slots(
                                                    ui,
                                                    &mut inserts,
                                                ) {
                                                    changes.push((
                                                        *track_id,
                                                        inserts.clone(),
                                                    ));
                                                }
                                            },
                                        );
                                        if after != before {
                                            let merge = after.mute
                                                == before.mute
                                                && after.solo == before.solo
                                                && after.record_arm
                                                    == before.record_arm
                                                && after.monitor_mode
                                                    == before.monitor_mode;
                                            track_changes.push((
                                                *track_id,
                                                before,
                                                after,
                                                merge,
                                            ));
                                        }
                                    });
                                });
                                ui.add_space(6.0);
                            }
                        });
                    });
            });

        if !open {
            self.fx_rack_open = false;
            self.commit_pending_insert_edit();
        }
        if let Some(chain) = copy_chain {
            self.insert_clipboard = Some(chain);
        }
        for (track_id, from, to, merge) in track_changes {
            if let Some(track) = self.project.tracks.get_mut(&track_id) {
                to.apply(track);
            }
            self.push_undo(EditAction::TrackState { track_id, from, to }, merge);
            self.after_edit();
            let tracks = self.collect_playable_tracks();
            self.engine.set_playback_tracks(tracks);
        }
        for (track_id, inserts) in changes {
            self.apply_track_inserts(track_id, inserts);
        }
    }

    /// Editable piano-roll view: pitch stripes, beat/bar grid, note selection,
    /// drag move/resize, double-click entry and right-click delete.
    fn show_piano_roll(&mut self, ctx: &egui::Context) {
        let Some(mut window) = self.piano_roll.clone() else {
            return;
        };
        let Some(clip) = self.project.midi_clips.get(window.clip_id).cloned() else {
            self.stop_piano_roll_preview();
            self.piano_roll = None;
            return;
        };
        if !window.initialized {
            window.load(&clip);
        }

        // The editor works in content coordinates: trimmed-away notes stay in
        // the buffer so dragging the clip edge back out can reveal them again.
        let visible_start = clip.content_offset;
        let visible_end = visible_start.saturating_add(clip.length.max(1));
        let content_end = window
            .notes
            .iter()
            .map(|note| note.end)
            .max()
            .unwrap_or(0)
            .max(visible_end)
            .max(1);
        let length = content_end;
        let (numerator, _) = self.time_signature();
        let beats_per_bar = numerator.max(1) as u64;
        let beat_ticks = 960u64;
        let bar_ticks = beat_ticks * beats_per_bar.max(1);
        let sample_rate = self.project.sample_rate.max(1) as f64;
        let playhead_sample =
            (self.playhead_secs.max(0.0) * sample_rate).round() as u64;
        let playhead_tick = self
            .project
            .tempo_map
            .sample_to_tick(playhead_sample, sample_rate);
        let playhead_content = playhead_tick as i64
            - clip.timeline_position.tick as i64
            + clip.content_offset as i64;
        let playhead_content_tick =
            (playhead_content >= 0).then_some(playhead_content as u64);
        let ghost_notes = if window.show_ghost_notes {
            ghost_notes_for_clip(&self.project, window.clip_id, length)
        } else {
            Vec::new()
        };
        let mut open = true;
        let mut close = false;
        let mut commit = false;
        let mut undo_before: Option<MidiClip> = None;
        let mut delete_selected = false;
        let mut select_all = false;
        let mut preview_note: Option<(u8, u8)> = None;
        let mut zoom_request: Option<f32> = None;
        let mut fit_selection = false;
        let mut do_quantize = false;
        let mut do_transpose = false;
        let mut do_scale = false;
        let mut do_velocity = false;
        let mut do_humanize = false;
        let mut do_legato = false;
        let mut do_note_length = false;
        let mut do_join = false;
        let mut do_split = false;
        let mut do_reverse = false;
        let mut do_mirror = false;
        let mut do_randomize = false;
        let mut cc_undo_before: Option<MidiClip> = None;
        let mut seek_content_tick: Option<u64> = None;

        egui::Window::new("Piano Roll")
            .id(egui::Id::new(("piano_roll", window.clip_id)))
            .open(&mut open)
            .default_size([1180.0, 700.0])
            .min_size([720.0, 600.0])
            .show(ctx, |ui| {
                let content_bottom = ui.max_rect().bottom();
                let mut display_playhead_tick = playhead_content_tick;
                ui.horizontal(|ui| {
                    ui.label(format!("{} notes", window.notes.len()));
                    ui.separator();
                    ui.weak(format!(
                        "content {} · visible {} ticks",
                        length, clip.length
                    ));
                    ui.checkbox(&mut window.show_ghost_notes, "Ghosts")
                        .on_hover_text(
                            "Show notes from other tracks/clips as timing references",
                        );
                    ui.checkbox(&mut window.show_scale_highlight, "Scale")
                        .on_hover_text(format!(
                            "Highlight rows in {} {}",
                            PITCH_CLASS_NAMES[window.tools.scale_root as usize],
                            window.tools.scale_type.label()
                        ));
                    ui.checkbox(&mut window.drum_mode, "Drum")
                        .on_hover_text(
                            "Label every pitch row by note name instead of showing piano keys",
                        );
                    ui.separator();
                    for tool in PianoRollTool::ALL {
                        if ui
                            .selectable_label(window.tool == tool, tool.label())
                            .on_hover_text(tool.hint())
                            .clicked()
                        {
                            window.tool = tool;
                        }
                    }
                    ui.separator();
                    ui.label("Snap:");
                    egui::ComboBox::from_id_salt("piano_roll_snap")
                        .selected_text(window.snap.label())
                        .width(76.0)
                        .show_ui(ui, |ui| {
                            for snap in PianoRollSnap::ALL {
                                ui.selectable_value(&mut window.snap, snap, snap.label());
                            }
                        });
                    if ui.small_button("Fit").clicked() {
                        window.needs_fit = true;
                    }
                    if ui
                        .add_enabled(
                            !window.selected.is_empty(),
                            egui::Button::new("Fit Sel"),
                        )
                        .on_hover_text("Fit the view to selected notes")
                        .clicked()
                    {
                        fit_selection = true;
                    }
                    if ui.small_button("−").clicked() {
                        zoom_request = Some(1.0 / 1.5);
                    }
                    if ui.small_button("+").clicked() {
                        zoom_request = Some(1.5);
                    }
                    if ui
                        .add_enabled(
                            !window.selected.is_empty(),
                            egui::Button::new("Delete"),
                        )
                        .clicked()
                    {
                        delete_selected = true;
                    }
                    if ui.small_button("Select All").clicked() {
                        select_all = true;
                    }
                    if ui.small_button("Close").clicked() {
                        close = true;
                    }
                });
                ui.weak(
                    "Double-click add · drag move/resize · right-click delete · wheel pan · Ctrl+wheel zoom",
                );
                let snap_ticks = window.snap.ticks(beats_per_bar);
                // Size the editor from the window's allocated height, not from
                // the content's available height, so egui's auto-resize cannot
                // feed back into itself. The velocity lane grows with the
                // window instead of staying a fixed strip.
                let editor_height = (content_bottom
                    - ui.cursor().top()
                    - ui.spacing().item_spacing.y
                    - 12.0)
                    .max(220.0);
                let velocity_lane_height = if window.show_velocity_lane {
                    (editor_height * 0.22).clamp(100.0, 180.0)
                } else {
                    0.0
                };
                let cc_lane_count = window.cc_lanes.len();
                let cc_available = (editor_height
                    - velocity_lane_height
                    - 120.0)
                    .max(0.0);
                let cc_lane_height = if cc_lane_count > 0 {
                    (cc_available / cc_lane_count as f32).clamp(40.0, 70.0)
                } else {
                    0.0
                };
                let cc_total_height = cc_lane_height * cc_lane_count as f32;
                let visible_lanes =
                    window.show_velocity_lane as u8 + cc_lane_count as u8;
                let lane_spacing = 6.0 * visible_lanes as f32;

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        egui::ScrollArea::vertical()
                            .max_height(editor_height)
                            .show(ui, |ui| {
                        ui.heading("MIDI Tools");
                        ui.weak("Applies to this clip.");
                        ui.menu_button("Tools…", |ui| {
                            for tool in MidiToolTab::ALL {
                                let mut visible =
                                    window.tools.visible_tools.contains(&tool);
                                if ui.checkbox(&mut visible, tool.label()).changed() {
                                    if visible {
                                        window.tools.visible_tools.insert(tool);
                                    } else {
                                        window.tools.visible_tools.remove(&tool);
                                    }
                                }
                            }
                        });
                        ui.separator();
                        if window.tools.visible_tools.is_empty() {
                            ui.weak("Choose tools from the Tools menu.");
                        }
                        for tool in MidiToolTab::ALL {
                            if !window.tools.visible_tools.contains(&tool) {
                                continue;
                            }
                            egui::CollapsingHeader::new(tool.label())
                                .default_open(true)
                                .show(ui, |ui| {
                        match tool {
                            MidiToolTab::Quantize => {
                                ui.weak("Move note starts toward the grid.");
                                ui.horizontal(|ui| {
                                    ui.label("Grid:");
                                    egui::ComboBox::from_id_salt(
                                        "piano_roll_quantize_grid",
                                    )
                                    .selected_text(window.tools.quantize_snap.label())
                                    .width(76.0)
                                    .show_ui(ui, |ui| {
                                        for snap in PianoRollSnap::ALL {
                                            if snap != PianoRollSnap::Off {
                                                ui.selectable_value(
                                                    &mut window.tools.quantize_snap,
                                                    snap,
                                                    snap.label(),
                                                );
                                            }
                                        }
                                    });
                                });
                                let mut strength =
                                    window.tools.quantize_strength * 100.0;
                                if add_wheel_slider(
                                    ui,
                                    &mut strength,
                                    0.0..=100.0,
                                    |slider| {
                                        slider.text("Strength").suffix("%")
                                    },
                                )
                                .changed()
                                {
                                    window.tools.quantize_strength =
                                        strength / 100.0;
                                }
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.quantize_swing,
                                    0.0..=100.0,
                                    |slider| {
                                        slider.text("Swing").suffix("%")
                                    },
                                );
                                if ui
                                    .add_enabled(
                                        window.tools.quantize_snap
                                            != PianoRollSnap::Off
                                            && window.tools.quantize_strength
                                                > 0.0,
                                        egui::Button::new("Quantize"),
                                    )
                                    .clicked()
                                {
                                    do_quantize = true;
                                }
                            }
                            MidiToolTab::Transpose => {
                                ui.weak("Shift note pitches.");
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.transpose,
                                    -24..=24,
                                    |slider| {
                                        slider
                                            .text("Semitones")
                                            .step_by(1.0)
                                    },
                                );
                                ui.horizontal(|ui| {
                                    if ui.small_button("-12").clicked() {
                                        window.tools.transpose -= 12;
                                    }
                                    if ui.small_button("+12").clicked() {
                                        window.tools.transpose += 12;
                                    }
                                    window.tools.transpose =
                                        window.tools.transpose.clamp(-24, 24);
                                });
                                if ui
                                    .add_enabled(
                                        window.tools.transpose != 0,
                                        egui::Button::new("Transpose"),
                                    )
                                    .clicked()
                                {
                                    do_transpose = true;
                                }
                            }
                            MidiToolTab::Scale => {
                                ui.weak("Snap notes to a musical scale.");
                                ui.horizontal(|ui| {
                                    ui.label("Root:");
                                    egui::ComboBox::from_id_salt(
                                        "piano_roll_scale_root",
                                    )
                                    .selected_text(
                                        PITCH_CLASS_NAMES
                                            [window.tools.scale_root as usize],
                                    )
                                    .width(60.0)
                                    .show_ui(ui, |ui| {
                                        for (index, name) in
                                            PITCH_CLASS_NAMES.iter().enumerate()
                                        {
                                            ui.selectable_value(
                                                &mut window.tools.scale_root,
                                                index as u8,
                                                *name,
                                            );
                                        }
                                    });
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Scale:");
                                    egui::ComboBox::from_id_salt(
                                        "piano_roll_scale_type",
                                    )
                                    .selected_text(window.tools.scale_type.label())
                                    .width(130.0)
                                    .show_ui(ui, |ui| {
                                        for scale in ScaleType::ALL {
                                            ui.selectable_value(
                                                &mut window.tools.scale_type,
                                                scale,
                                                scale.label(),
                                            );
                                        }
                                    });
                                });
                                if ui.button("Conform").clicked() {
                                    do_scale = true;
                                }
                            }
                            MidiToolTab::Humanize => {
                                ui.weak("Add subtle timing and velocity variation.");
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.humanize_timing,
                                    0.0..=240.0,
                                    |slider| {
                                        slider
                                            .text("Timing")
                                            .suffix(" ticks")
                                    },
                                );
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.humanize_velocity,
                                    0.0..=64.0,
                                    |slider| slider.text("Velocity"),
                                );
                                if ui
                                    .add_enabled(
                                        window.tools.humanize_timing > 0.0
                                            || window.tools.humanize_velocity
                                                > 0.0,
                                        egui::Button::new("Humanize"),
                                    )
                                    .clicked()
                                {
                                    do_humanize = true;
                                }
                            }
                            MidiToolTab::Legato => {
                                ui.weak("Extend notes toward the next note.");
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.legato_gap,
                                    0.0..=240.0,
                                    |slider| {
                                        slider.text("Gap").suffix(" ticks")
                                    },
                                );
                                if ui.button("Legato").clicked() {
                                    do_legato = true;
                                }
                            }
                            MidiToolTab::NoteLength => {
                                ui.weak("Set every note to a fixed length.");
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.note_length,
                                    30.0..=3840.0,
                                    |slider| {
                                        slider
                                            .text("Length")
                                            .suffix(" ticks")
                                    },
                                );
                                if ui.button("Set Length").clicked() {
                                    do_note_length = true;
                                }
                            }
                            MidiToolTab::Notes => {
                                ui.weak("Join or split notes.");
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.join_gap,
                                    0.0..=960.0,
                                    |slider| {
                                        slider
                                            .text("Join gap")
                                            .suffix(" ticks")
                                            .step_by(1.0)
                                    },
                                );
                                if ui
                                    .add_enabled(
                                        window.notes.len() >= 2,
                                        egui::Button::new("Join Notes"),
                                    )
                                    .clicked()
                                {
                                    do_join = true;
                                }
                                ui.separator();
                                ui.weak("Split notes crossing the playhead.");
                                match playhead_content_tick {
                                    Some(tick) if tick < length => {
                                        ui.weak(format!(
                                            "Playhead: {} ticks",
                                            tick
                                        ));
                                    }
                                    _ => {
                                        ui.weak("Playhead is outside this clip.");
                                    }
                                }
                                let split_available = playhead_content_tick
                                    .map_or(false, |tick| {
                                        tick > 0 && tick < length
                                    });
                                if ui
                                    .add_enabled(
                                        split_available,
                                        egui::Button::new("Split at Playhead"),
                                    )
                                    .clicked()
                                {
                                    do_split = true;
                                }
                            }
                            MidiToolTab::Transform => {
                                ui.weak("Reverse, mirror, thin or randomize notes.");
                                if ui.button("Reverse").clicked() {
                                    do_reverse = true;
                                }
                                ui.separator();
                                let center = window.tools.mirror_center;
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.mirror_center,
                                    0..=127,
                                    |slider| {
                                        slider.text(format!(
                                            "Mirror center ({})",
                                            note_name(center)
                                        ))
                                    },
                                );
                                if ui.button("Mirror Pitch").clicked() {
                                    do_mirror = true;
                                }
                                ui.separator();
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.randomize_pitch,
                                    0.0..=12.0,
                                    |slider| {
                                        slider.text("Pitch ±").step_by(1.0)
                                    },
                                );
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.randomize_velocity,
                                    0.0..=64.0,
                                    |slider| {
                                        slider
                                            .text("Velocity ±")
                                            .step_by(1.0)
                                    },
                                );
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.randomize_timing,
                                    0.0..=240.0,
                                    |slider| {
                                        slider
                                            .text("Timing ±")
                                            .suffix(" ticks")
                                            .step_by(1.0)
                                    },
                                );
                                add_wheel_slider(
                                    ui,
                                    &mut window.tools.randomize_density,
                                    0.0..=100.0,
                                    |slider| {
                                        slider
                                            .text("Thin")
                                            .suffix("%")
                                            .step_by(1.0)
                                    },
                                );
                                if ui
                                    .add_enabled(
                                        window.tools.randomize_pitch > 0.0
                                            || window.tools.randomize_velocity > 0.0
                                            || window.tools.randomize_timing > 0.0
                                            || window.tools.randomize_density > 0.0,
                                        egui::Button::new("Randomize"),
                                    )
                                    .clicked()
                                {
                                    do_randomize = true;
                                }
                            }
                            MidiToolTab::Velocity => {
                                ui.weak("Shape note velocities.");
                                ui.checkbox(
                                    &mut window.show_velocity_lane,
                                    "Show velocity lane",
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Mode:");
                                    egui::ComboBox::from_id_salt(
                                        "piano_roll_velocity_mode",
                                    )
                                    .selected_text(
                                        window.tools.velocity_mode.label(),
                                    )
                                    .width(96.0)
                                    .show_ui(ui, |ui| {
                                        for mode in VelocityMode::ALL {
                                            ui.selectable_value(
                                                &mut window.tools.velocity_mode,
                                                mode,
                                                mode.label(),
                                            );
                                        }
                                    });
                                });
                                ui.weak(
                                    window.tools.velocity_mode.description(),
                                );
                                match window.tools.velocity_mode {
                                    VelocityMode::Scale => {
                                        let mut velocity_percent =
                                            window.tools.velocity_factor
                                                * 100.0;
                                        if add_wheel_slider_step(
                                            ui,
                                            &mut velocity_percent,
                                            0.0..=200.0,
                                            Some(5.0),
                                            false,
                                            |slider| {
                                                slider
                                                    .text("Scale")
                                                    .suffix("%")
                                            },
                                        )
                                        .changed()
                                        {
                                            window.tools.velocity_factor =
                                                velocity_percent / 100.0;
                                        }
                                    }
                                    VelocityMode::Compress => {
                                        add_wheel_slider(
                                            ui,
                                            &mut window.tools.velocity_amount,
                                            0.0..=100.0,
                                            |slider| {
                                                slider
                                                    .text("Amount")
                                                    .suffix("%")
                                            },
                                        );
                                    }
                                    VelocityMode::Expand => {
                                        add_wheel_slider(
                                            ui,
                                            &mut window.tools.velocity_amount,
                                            0.0..=100.0,
                                            |slider| {
                                                slider
                                                    .text("Amount")
                                                    .suffix("%")
                                            },
                                        );
                                    }
                                    VelocityMode::Curve => {
                                        add_wheel_slider(
                                            ui,
                                            &mut window.tools.velocity_curve,
                                            -100.0..=100.0,
                                            |slider| slider.text("Curve"),
                                        );
                                    }
                                    VelocityMode::Randomize => {
                                        add_wheel_slider(
                                            ui,
                                            &mut window.tools.velocity_random,
                                            0.0..=64.0,
                                            |slider| {
                                                slider
                                                    .text("Amount")
                                                    .step_by(1.0)
                                            },
                                        );
                                    }
                                }
                                let can_apply = match window.tools.velocity_mode {
                                    VelocityMode::Scale => {
                                        (window.tools.velocity_factor - 1.0)
                                            .abs()
                                            > 1e-3
                                    }
                                    VelocityMode::Compress
                                    | VelocityMode::Expand => {
                                        window.tools.velocity_amount > 0.0
                                    }
                                    VelocityMode::Curve => {
                                        window.tools.velocity_curve.abs() > 0.1
                                    }
                                    VelocityMode::Randomize => {
                                        window.tools.velocity_random > 0.0
                                    }
                                };
                                if ui
                                    .add_enabled(
                                        can_apply,
                                        egui::Button::new("Apply Velocity"),
                                    )
                                    .clicked()
                                {
                                    do_velocity = true;
                                }
                            }
                            MidiToolTab::Controllers => {
                                ui.weak("Draw controller automation.");
                                if window.cc_lanes.is_empty() {
                                    ui.weak("No CC lanes");
                                }
                                let mut remove_lane = None;
                                for (index, controller) in
                                    window.cc_lanes.iter().enumerate()
                                {
                                    ui.horizontal(|ui| {
                                        ui.label(format!(
                                            "CC {} · {}",
                                            controller,
                                            cc_controller_label(*controller)
                                        ));
                                        if ui
                                            .small_button("×")
                                            .on_hover_text("Remove lane")
                                            .clicked()
                                        {
                                            remove_lane = Some(index);
                                        }
                                    });
                                }
                                if let Some(index) = remove_lane {
                                    window.cc_lanes.remove(index);
                                }
                                if window.cc_lanes.len() < 4 {
                                    ui.menu_button("+ Add Lane", |ui| {
                                        for (controller, label) in
                                            CC_CONTROLLERS
                                        {
                                            if window
                                                .cc_lanes
                                                .contains(&controller)
                                            {
                                                continue;
                                            }
                                            if ui
                                                .button(format!(
                                                    "CC {} · {}",
                                                    controller, label
                                                ))
                                                .clicked()
                                            {
                                                window.cc_lanes.push(controller);
                                                window.cc_lanes.sort_unstable();
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                }
                            }
                        }
                                });
                            ui.add_space(4.0);
                        }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {

                let grid_height = (editor_height
                    - velocity_lane_height
                    - cc_total_height
                    - lane_spacing)
                    .max(120.0);
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(ui.available_width(), grid_height),
                    egui::Sense::click_and_drag(),
                );
                let rect = response.rect;
                if rect.width() < 80.0 || rect.height() < 40.0 {
                    return;
                }

                let keyboard_width = 48.0;
                let ruler_height = 18.0;
                let scrollbar_size = 8.0;
                let ruler = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + keyboard_width, rect.top()),
                    egui::pos2(rect.right() - scrollbar_size, rect.top() + ruler_height),
                );
                let keyboard = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + ruler_height),
                    egui::pos2(
                        rect.left() + keyboard_width,
                        rect.bottom() - scrollbar_size,
                    ),
                );
                let grid = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + keyboard_width, rect.top() + ruler_height),
                    egui::pos2(
                        rect.right() - scrollbar_size,
                        rect.bottom() - scrollbar_size,
                    ),
                );
                let h_scroll = egui::Rect::from_min_max(
                    egui::pos2(grid.left(), grid.bottom()),
                    egui::pos2(grid.right(), rect.bottom()),
                );
                let v_scroll = egui::Rect::from_min_max(
                    egui::pos2(grid.right(), grid.top()),
                    egui::pos2(rect.right(), grid.bottom()),
                );
                let ruler_response = ui
                    .interact(
                        ruler,
                        ui.id().with(("piano_roll_ruler", window.clip_id)),
                        egui::Sense::click_and_drag(),
                    )
                    .on_hover_text("Drag to move the playhead");

                if fit_selection {
                    fit_piano_roll_selection(&mut window, grid);
                } else if window.needs_fit {
                    fit_piano_roll_view(&mut window, grid, length);
                }
                if let Some(factor) = zoom_request {
                    let old = window.pixels_per_tick.max(0.0001);
                    let new = (old * factor).clamp(0.0005, 4.0);
                    let center_tick = window.view_start_tick
                        + (grid.width() * 0.5) as f64 / old as f64;
                    window.pixels_per_tick = new;
                    window.view_start_tick =
                        center_tick - (grid.width() * 0.5) as f64 / new as f64;
                }

                // Pan and zoom the view like the arrange timeline.
                let hovered = ui.rect_contains_pointer(grid);
                let (zoom_delta, scroll, view_pointer, middle_delta, view_modifiers) =
                    ui.input(|i| {
                        (
                            i.zoom_delta(),
                            i.smooth_scroll_delta,
                            i.pointer.hover_pos(),
                            if i.pointer.button_down(egui::PointerButton::Middle) {
                                i.pointer.delta()
                            } else {
                                egui::Vec2::ZERO
                            },
                            i.modifiers,
                        )
                    });
                if hovered {
                    if (zoom_delta - 1.0).abs() > 0.001 {
                        let px = view_pointer
                            .map(|pos| pos.x)
                            .unwrap_or(grid.center().x);
                        let py = view_pointer
                            .map(|pos| pos.y)
                            .unwrap_or(grid.center().y);
                        if view_modifiers.shift {
                            let old = window.pixels_per_semitone;
                            let new = (old * zoom_delta).clamp(4.0, 40.0);
                            let anchor = window.view_low_pitch
                                + (grid.bottom() - py) / old.max(0.0001);
                            window.pixels_per_semitone = new;
                            window.view_low_pitch =
                                anchor - (grid.bottom() - py) / new.max(0.0001);
                        } else {
                            let old = window.pixels_per_tick;
                            let new = (old * zoom_delta).clamp(0.0005, 4.0);
                            let anchor = window.view_start_tick
                                + (px - grid.left()) as f64 / old.max(0.0001) as f64;
                            window.pixels_per_tick = new;
                            window.view_start_tick =
                                anchor - (px - grid.left()) as f64 / new.max(0.0001) as f64;
                        }
                    } else if scroll.x != 0.0 || scroll.y != 0.0 {
                        // Wheel scrolls time like the arrange view; Shift+wheel
                        // scrolls pitch. egui turns Shift+wheel into horizontal
                        // scroll, so both axes contribute.
                        if view_modifiers.shift {
                            window.view_low_pitch += (scroll.x + scroll.y)
                                / window.pixels_per_semitone.max(0.0001);
                        } else {
                            window.view_start_tick -= (scroll.x + scroll.y) as f64
                                / window.pixels_per_tick.max(0.0001) as f64;
                        }
                    }
                }
                if hovered && middle_delta != egui::Vec2::ZERO {
                    window.view_start_tick -= middle_delta.x as f64
                        / window.pixels_per_tick.max(0.0001) as f64;
                    window.view_low_pitch +=
                        middle_delta.y / window.pixels_per_semitone.max(0.0001);
                }

                // Clamp the view to the clip and the MIDI pitch range.
                let visible_ticks =
                    grid.width() / window.pixels_per_tick.max(0.0001);
                let visible_pitches =
                    (grid.height() / window.pixels_per_semitone.max(1.0)).max(1.0);

                // Horizontal and vertical scrollbars.
                let h_max = (length as f64 - visible_ticks as f64).max(0.0);
                let h_thumb_width = if length == 0 {
                    grid.width()
                } else {
                    (grid.width() * (visible_ticks / length as f32))
                        .clamp(24.0_f32.min(grid.width()), grid.width())
                };
                let h_travel = (grid.width() - h_thumb_width).max(1.0);
                let h_scroll_response = ui.interact(
                    h_scroll,
                    ui.id().with(("piano_roll_h_scroll", window.clip_id)),
                    egui::Sense::click_and_drag(),
                );
                if h_scroll_response.dragged() {
                    let delta = ui.input(|i| i.pointer.delta());
                    if h_max > 0.0 {
                        window.view_start_tick += delta.x as f64
                            * (h_max / h_travel.max(1.0) as f64);
                    }
                } else if h_scroll_response.clicked() {
                    if let Some(pos) = h_scroll_response.interact_pointer_pos() {
                        let fraction = ((pos.x - grid.left() - h_thumb_width * 0.5)
                            / h_travel)
                            .clamp(0.0, 1.0);
                        window.view_start_tick = fraction as f64 * h_max;
                    }
                }
                let h_scroll_active =
                    h_scroll_response.hovered() || h_scroll_response.dragged();

                let v_max = (128.0 - visible_pitches).max(0.0);
                let v_thumb_height = (grid.height()
                    * (visible_pitches / 128.0))
                    .clamp(24.0_f32.min(grid.height()), grid.height());
                let v_travel = (grid.height() - v_thumb_height).max(1.0);
                let v_scroll_response = ui.interact(
                    v_scroll,
                    ui.id().with(("piano_roll_v_scroll", window.clip_id)),
                    egui::Sense::click_and_drag(),
                );
                if v_scroll_response.dragged() {
                    let delta = ui.input(|i| i.pointer.delta());
                    if v_max > 0.0 {
                        window.view_low_pitch -= delta.y
                            * (v_max / v_travel.max(1.0));
                    }
                } else if v_scroll_response.clicked() {
                    if let Some(pos) = v_scroll_response.interact_pointer_pos() {
                        let fraction = ((pos.y - grid.top() - v_thumb_height * 0.5)
                            / v_travel)
                            .clamp(0.0, 1.0);
                        window.view_low_pitch = (1.0 - fraction) * v_max;
                    }
                }
                let v_scroll_active =
                    v_scroll_response.hovered() || v_scroll_response.dragged();

                window.view_start_tick =
                    window.view_start_tick.clamp(0.0, h_max);
                window.view_low_pitch = window
                    .view_low_pitch
                    .clamp(0.0, v_max);

                let low = window.view_low_pitch.floor().clamp(0.0, 127.0) as u8;
                let high = (window.view_low_pitch + visible_pitches)
                    .ceil()
                    .clamp(0.0, 127.0) as u8;
                let row_height = window.pixels_per_semitone;
                let y_for_pitch = |pitch: f32| -> f32 {
                    grid.bottom() - (pitch - window.view_low_pitch) * row_height
                };
                let x_for_tick = |tick: f64| -> f32 {
                    grid.left()
                        + (tick - window.view_start_tick) as f32
                            * window.pixels_per_tick
                };

                let playhead_x = playhead_content_tick
                    .map(|tick| x_for_tick(tick as f64))
                    .filter(|x| *x >= grid.left() && *x <= grid.right());
                let playhead_response = playhead_x.map(|x| {
                    ui.interact(
                        egui::Rect::from_min_max(
                            egui::pos2(x - 6.0, ruler.top()),
                            egui::pos2(x + 6.0, grid.bottom()),
                        ),
                        ui.id().with(("piano_roll_playhead", window.clip_id)),
                        egui::Sense::drag(),
                    )
                    .on_hover_text("Drag to move the playhead")
                });

                painter.rect_filled(grid, 0.0, egui::Color32::from_gray(28));
                painter.rect_filled(ruler, 0.0, egui::Color32::from_gray(22));
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(keyboard.right(), ruler.bottom()),
                    ),
                    0.0,
                    egui::Color32::from_gray(18),
                );
                painter.line_segment(
                    [
                        egui::pos2(ruler.left(), ruler.bottom()),
                        egui::pos2(ruler.right(), ruler.bottom()),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(70)),
                );
                for pitch in low..=high {
                    let y1 = y_for_pitch(pitch as f32);
                    let y0 = y1 - row_height;
                    let row = egui::Rect::from_min_max(
                        egui::pos2(grid.left(), y0.max(grid.top())),
                        egui::pos2(grid.right(), y1.min(grid.bottom())),
                    );
                    if row.height() <= 0.0 {
                        continue;
                    }
                    let black = is_black_key(pitch);
                    let drum = window.drum_mode;
                    let in_scale = window.show_scale_highlight
                        && pitch_in_scale(
                            pitch,
                            window.tools.scale_root,
                            window.tools.scale_type.intervals(),
                        );
                    let row_color = if drum {
                        if pitch % 4 == 0 {
                            egui::Color32::from_gray(34)
                        } else {
                            egui::Color32::from_gray(28)
                        }
                    } else if black {
                        egui::Color32::from_gray(24)
                    } else {
                        egui::Color32::from_gray(36)
                    };
                    painter.rect_filled(row, 0.0, row_color);
                    if in_scale {
                        painter.rect_filled(
                            row,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(
                                80, 180, 120, 22,
                            ),
                        );
                    }

                    let key = egui::Rect::from_min_max(
                        egui::pos2(keyboard.left(), y0.max(grid.top())),
                        egui::pos2(keyboard.right(), y1.min(grid.bottom())),
                    );
                    let key_color = if drum {
                        if pitch % 4 == 0 {
                            egui::Color32::from_gray(55)
                        } else {
                            egui::Color32::from_gray(38)
                        }
                    } else if in_scale {
                        if black {
                            egui::Color32::from_rgb(40, 72, 56)
                        } else {
                            egui::Color32::from_rgb(188, 224, 202)
                        }
                    } else if black {
                        egui::Color32::from_gray(18)
                    } else {
                        egui::Color32::from_gray(205)
                    };
                    painter.rect_filled(key, 0.0, key_color);
                    if drum || pitch % 12 == 0 {
                        painter.text(
                            egui::pos2(keyboard.left() + 2.0, key.center().y),
                            egui::Align2::LEFT_CENTER,
                            note_name(pitch),
                            egui::FontId::monospace(8.0),
                            if drum {
                                egui::Color32::from_gray(210)
                            } else if black {
                                egui::Color32::from_gray(230)
                            } else {
                                egui::Color32::from_gray(20)
                            },
                        );
                    }
                }

                // Shade material trimmed away from the clip bounds so hidden
                // notes remain visible and can be recovered by extending the
                // clip edge back out.
                let trim_start_x = x_for_tick(visible_start as f64);
                let trim_end_x = x_for_tick(visible_end as f64);
                let shade = egui::Color32::from_rgba_unmultiplied(10, 10, 14, 130);
                if visible_start > 0 {
                    let left = x_for_tick(0.0).max(grid.left());
                    let right = trim_start_x.min(grid.right());
                    if right > left {
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(left, grid.top()),
                                egui::pos2(right, grid.bottom()),
                            ),
                            0.0,
                            shade,
                        );
                    }
                }
                if visible_end < length {
                    let left = trim_end_x.max(grid.left());
                    let right = grid.right();
                    if right > left {
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(left, grid.top()),
                                egui::pos2(right, grid.bottom()),
                            ),
                            0.0,
                            shade,
                        );
                    }
                }
                for x in [trim_start_x, trim_end_x] {
                    if x >= grid.left() && x <= grid.right() {
                        painter.line_segment(
                            [egui::pos2(x, grid.top()), egui::pos2(x, grid.bottom())],
                            egui::Stroke::new(
                                1.0_f32,
                                egui::Color32::from_rgba_unmultiplied(220, 220, 160, 160),
                            ),
                        );
                    }
                }

                // Snap subdivisions first, then beats and bars on top.
                let visible_start = window.view_start_tick.max(0.0);
                let visible_end = (window.view_start_tick + visible_ticks as f64)
                    .min(length as f64);
                if snap_ticks > 1 && visible_ticks / snap_ticks as f32 <= 500.0 {
                    let mut tick = ((visible_start as u64) / snap_ticks) * snap_ticks;
                    while (tick as f64) <= visible_end {
                        if tick % beat_ticks != 0 {
                            let x = x_for_tick(tick as f64);
                            painter.line_segment(
                                [egui::pos2(x, grid.top()), egui::pos2(x, grid.bottom())],
                                egui::Stroke::new(0.5_f32, egui::Color32::from_gray(42)),
                            );
                        }
                        tick += snap_ticks;
                    }
                }
                let mut tick = ((visible_start as u64) / beat_ticks) * beat_ticks;
                while (tick as f64) <= visible_end {
                    let x = x_for_tick(tick as f64);
                    let is_bar = tick % bar_ticks == 0;
                    painter.line_segment(
                        [egui::pos2(x, grid.top()), egui::pos2(x, grid.bottom())],
                        egui::Stroke::new(
                            if is_bar { 1.0_f32 } else { 0.5_f32 },
                            if is_bar {
                                egui::Color32::from_gray(78)
                            } else {
                                egui::Color32::from_gray(52)
                            },
                        ),
                    );
                    tick += beat_ticks;
                }

                let mut ruler_tick =
                    ((window.view_start_tick.max(0.0) as u64) / beat_ticks) * beat_ticks;
                let bar_width = bar_ticks as f32 * window.pixels_per_tick;
                while (ruler_tick as f64)
                    <= window.view_start_tick + visible_ticks as f64
                {
                    let x = x_for_tick(ruler_tick as f64);
                    let is_bar = ruler_tick % bar_ticks == 0;
                    painter.line_segment(
                        [
                            egui::pos2(
                                x,
                                if is_bar {
                                    ruler.top() + 4.0
                                } else {
                                    ruler.bottom() - 5.0
                                },
                            ),
                            egui::pos2(x, ruler.bottom()),
                        ],
                        egui::Stroke::new(
                            if is_bar { 1.0_f32 } else { 0.5_f32 },
                            egui::Color32::from_gray(if is_bar { 120 } else { 75 }),
                        ),
                    );
                    if is_bar && bar_width >= 24.0 {
                        painter.text(
                            egui::pos2(x + 3.0, ruler.top() + 1.0),
                            egui::Align2::LEFT_TOP,
                            format!("{}", ruler_tick / bar_ticks + 1),
                            egui::FontId::monospace(8.0),
                            egui::Color32::from_gray(170),
                        );
                    }
                    ruler_tick += beat_ticks;
                }

                let pitch_at = |y: f32| -> u8 {
                    let y = y.clamp(grid.top(), grid.bottom() - 0.001);
                    let pitch = window.view_low_pitch
                        + (grid.bottom() - y) / row_height.max(0.0001);
                    pitch.floor().clamp(0.0, 127.0) as u8
                };
                let tick_at = |x: f32| -> u64 {
                    let tick = window.view_start_tick
                        + ((x.clamp(grid.left(), grid.right()) - grid.left()) as f64
                            / window.pixels_per_tick.max(0.0001) as f64);
                    tick.round().clamp(0.0, length as f64) as u64
                };
                let snap_tick = |tick: u64| -> u64 {
                    if snap_ticks <= 1 {
                        tick
                    } else {
                        ((tick + snap_ticks / 2) / snap_ticks) * snap_ticks
                    }
                };

                if let Some(response) = &playhead_response {
                    if response.dragged_by(egui::PointerButton::Primary) {
                        if let Some(pos) = response.interact_pointer_pos() {
                            seek_content_tick = Some(tick_at(pos.x));
                        }
                    }
                }
                if ruler_response.dragged_by(egui::PointerButton::Primary)
                    || ruler_response.clicked_by(egui::PointerButton::Primary)
                {
                    if let Some(pos) = ruler_response.interact_pointer_pos() {
                        seek_content_tick = Some(tick_at(pos.x));
                    }
                }
                if let Some(tick) = seek_content_tick {
                    display_playhead_tick = Some(tick);
                }

                for note in &ghost_notes {
                    if note.pitch < low || note.pitch > high {
                        continue;
                    }
                    let x0 = x_for_tick(note.start as f64);
                    let x1 = x_for_tick(note.end as f64);
                    if x1 < grid.left() || x0 > grid.right() {
                        continue;
                    }
                    let y1 = y_for_pitch(note.pitch as f32);
                    let y0 = y1 - row_height;
                    if y1 < grid.top() || y0 > grid.bottom() {
                        continue;
                    }
                    let left = x0.max(grid.left());
                    let right = x1.max(x0 + 2.0).min(grid.right());
                    let top = (y0 + 1.0).max(grid.top());
                    let bottom = (y1 - 1.0).min(grid.bottom()).max(top + 1.0);
                    let ghost_rect = egui::Rect::from_min_max(
                        egui::pos2(left, top),
                        egui::pos2(right.max(left + 1.0), bottom),
                    );
                    painter.rect_filled(
                        ghost_rect,
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(150, 165, 190, 55),
                    );
                    painter.rect_stroke(
                        ghost_rect,
                        2.0,
                        egui::Stroke::new(
                            0.5_f32,
                            egui::Color32::from_rgba_unmultiplied(180, 195, 220, 85),
                        ),
                    );
                }

                let mut note_rects: Vec<(usize, egui::Rect)> =
                    Vec::with_capacity(window.notes.len());
                for (index, note) in window.notes.iter().enumerate() {
                    if note.pitch < low || note.pitch > high {
                        continue;
                    }
                    let x0 = x_for_tick(note.start as f64);
                    let x1 = x_for_tick(note.end as f64);
                    if x1 < grid.left() || x0 > grid.right() {
                        continue;
                    }
                    let y1 = y_for_pitch(note.pitch as f32);
                    let y0 = y1 - row_height;
                    if y1 < grid.top() || y0 > grid.bottom() {
                        continue;
                    }
                    let left = x0.max(grid.left());
                    let right = x1.max(x0 + 2.0).min(grid.right());
                    let top = (y0 + 1.0).max(grid.top());
                    let bottom = (y1 - 1.0).min(grid.bottom()).max(top + 1.0);
                    let note_rect = egui::Rect::from_min_max(
                        egui::pos2(left, top),
                        egui::pos2(right.max(left + 1.0), bottom),
                    );
                    let selected = window.selected.contains(&index);
                    let color = velocity_color(note.velocity);
                    let fill = if selected {
                        egui::Color32::from_rgb(
                            color.r().saturating_add(35),
                            color.g().saturating_add(35),
                            color.b().saturating_add(35),
                        )
                    } else {
                        color
                    };
                    painter.rect_filled(note_rect, 2.0, fill);
                    painter.rect_stroke(
                        note_rect,
                        2.0,
                        egui::Stroke::new(
                            if selected { 1.5_f32 } else { 0.5_f32 },
                            if selected {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(230)
                            },
                        ),
                    );
                    note_rects.push((index, note_rect));
                }

                if let Some(tick) = display_playhead_tick {
                    let x = x_for_tick(tick as f64);
                    if x >= grid.left() && x <= grid.right() {
                        let color = egui::Color32::from_rgb(255, 100, 70);
                        painter.line_segment(
                            [
                                egui::pos2(x, ruler.top()),
                                egui::pos2(x, grid.bottom()),
                            ],
                            egui::Stroke::new(1.5_f32, color),
                        );
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(x - 5.0, ruler.top()),
                                egui::pos2(x + 5.0, ruler.top()),
                                egui::pos2(x, ruler.top() + 6.0),
                            ],
                            color,
                            egui::Stroke::NONE,
                        ));
                    }
                }

                let hover_pos = ui.input(|i| i.pointer.hover_pos());
                let hit_test = |pos: egui::Pos2| -> Option<(usize, NoteDragMode)> {
                    for (index, note_rect) in note_rects.iter().rev() {
                        if note_rect.contains(pos) {
                            let edge = (note_rect.right() - pos.x).max(0.0) <= 6.0;
                            return Some((
                                *index,
                                if edge {
                                    NoteDragMode::ResizeEnd
                                } else {
                                    NoteDragMode::Move
                                },
                            ));
                        }
                    }
                    None
                };
                let playhead_hit = |pos: egui::Pos2| -> bool {
                    playhead_x.map_or(false, |x| {
                        (pos.x - x).abs() <= 6.0
                            && pos.y >= ruler.top()
                            && pos.y <= grid.bottom()
                    })
                };

                if playhead_response
                    .as_ref()
                    .map_or(false, |response| response.hovered())
                    || ruler_response.hovered()
                {
                    ui.output_mut(|output| {
                        output.cursor_icon = egui::CursorIcon::ResizeHorizontal;
                    });
                } else if let Some(pos) = hover_pos {
                    if response.hovered() {
                        if window.tool == PianoRollTool::Velocity {
                            let icon = if hit_test(pos).is_some() {
                                egui::CursorIcon::ResizeVertical
                            } else {
                                egui::CursorIcon::Crosshair
                            };
                            ui.output_mut(|output| output.cursor_icon = icon);
                        } else if window.tool != PianoRollTool::Select {
                            ui.output_mut(|output| {
                                output.cursor_icon = egui::CursorIcon::Crosshair;
                            });
                        } else if let Some((_, mode)) = hit_test(pos) {
                            let icon = match mode {
                                NoteDragMode::Move => egui::CursorIcon::Grab,
                                NoteDragMode::ResizeEnd => {
                                    egui::CursorIcon::ResizeHorizontal
                                }
                                NoteDragMode::Velocity => {
                                    egui::CursorIcon::ResizeVertical
                                }
                            };
                            ui.output_mut(|output| output.cursor_icon = icon);
                        }
                    }
                }

                let modifiers = ui.input(|i| i.modifiers);
                let pointer_pos = response.interact_pointer_pos().or(hover_pos);
                let double_clicked = response.double_clicked();
                let mut handled_double = false;

                // Double-click empty space adds a note in Select mode. The
                // Draw tool adds one with a single click instead.
                if double_clicked
                    && window.tool == PianoRollTool::Select
                    && !modifiers.command
                    && !modifiers.shift
                {
                    if let Some(pos) = pointer_pos {
                        if grid.contains(pos)
                            && !playhead_hit(pos)
                            && hit_test(pos).is_none()
                            && clip.length > 0
                        {
                            let start = snap_tick(tick_at(pos.x))
                                .min(length.saturating_sub(1));
                            let wanted = piano_roll_new_note_length(
                                window.snap,
                                snap_ticks,
                                window.last_length,
                            );
                            window.last_length = wanted;
                            let end = start
                                .saturating_add(wanted)
                                .min(length)
                                .max(start + 1);
                            let note = PianoRollNote {
                                start,
                                end,
                                pitch: pitch_at(pos.y),
                                velocity: 100,
                                channel: 0,
                            };
                            undo_before = Some(clip.clone());
                            window.notes.push(note);
                            let index = window.notes.len() - 1;
                            window.selected.clear();
                            window.selected.insert(index);
                            preview_note = Some((
                                window.notes[index].pitch,
                                window.notes[index].velocity,
                            ));
                            commit = true;
                            handled_double = true;
                        }
                    }
                }

                // Right-click a note to delete it (or the whole selection if
                // the clicked note is selected).
                if response.secondary_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if let Some((index, _)) = hit_test(pos) {
                            undo_before = Some(clip.clone());
                            if window.selected.contains(&index) {
                                delete_selected = true;
                            } else {
                                window.notes.remove(index);
                                window.selected.clear();
                                window.drag = None;
                                commit = true;
                            }
                        }
                    }
                }

                if ui.rect_contains_pointer(ui.max_rect()) {
                    let delete_key = ui.input(|i| {
                        i.key_pressed(egui::Key::Delete)
                            || i.key_pressed(egui::Key::Backspace)
                    });
                    if delete_key && !window.selected.is_empty() {
                        delete_selected = true;
                    }
                    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
                        select_all = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        window.selected.clear();
                    }
                }

                if select_all {
                    window.selected = (0..window.notes.len()).collect();
                }
                if delete_selected && !window.selected.is_empty() {
                    undo_before = Some(clip.clone());
                    let selected = std::mem::take(&mut window.selected);
                    let mut index = 0usize;
                    window.notes.retain(|_| {
                        let keep = !selected.contains(&index);
                        index += 1;
                        keep
                    });
                    window.drag = None;
                    commit = true;
                }

                // Selection and drag start.
                if response.drag_started_by(egui::PointerButton::Primary) && !handled_double {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let press_pos = ui
                            .input(|i| i.pointer.press_origin())
                            .unwrap_or(pos);
                        if matches!(
                            window.tool,
                            PianoRollTool::Erase
                                | PianoRollTool::Split
                                | PianoRollTool::Glue
                        ) {
                            // These tools act on click; don't start a drag.
                        } else if h_scroll.contains(press_pos)
                            || v_scroll.contains(press_pos)
                        {
                            // Scrollbar drags are handled by their own widgets.
                        } else if ruler.contains(press_pos) {
                            // Ruler drags seek the playhead; leave the note
                            // selection untouched.
                        } else if playhead_hit(press_pos) {
                            // Dragging the playhead line seeks instead of
                            // editing notes.
                        } else if !grid.contains(press_pos) {
                            window.selected.clear();
                        } else {
                            match hit_test(press_pos) {
                                Some((index, mut mode)) => {
                                    if window.tool == PianoRollTool::Velocity {
                                        mode = NoteDragMode::Velocity;
                                    }
                                    if modifiers.command || modifiers.shift {
                                        if window.selected.contains(&index) {
                                            window.selected.remove(&index);
                                        } else {
                                            window.selected.insert(index);
                                        }
                                    } else if !window.selected.contains(&index) {
                                        window.selected.clear();
                                        window.selected.insert(index);
                                    }
                                    if window.selected.contains(&index) {
                                        if let Some(note) = window.notes.get(index) {
                                            preview_note = Some((note.pitch, note.velocity));
                                        }
                                        let originals = window
                                            .selected
                                            .iter()
                                            .filter_map(|index| {
                                                window
                                                    .notes
                                                    .get(*index)
                                                    .cloned()
                                                    .map(|note| (*index, note))
                                            })
                                            .collect();
                                        window.drag = Some(NoteDrag {
                                            mode,
                                            anchor_tick: tick_at(press_pos.x),
                                            anchor_pitch: pitch_at(press_pos.y),
                                            originals,
                                            preview_pitch: window
                                                .notes
                                                .get(index)
                                                .map(|note| note.pitch),
                                            before: clip.clone(),
                                        });
                                    }
                                }
                                None => {
                                    let additive =
                                        modifiers.command || modifiers.shift;
                                    window.marquee = Some((press_pos, press_pos));
                                    window.marquee_additive = additive;
                                    if !additive {
                                        window.selected.clear();
                                    }
                                }
                            }
                        }
                    }
                }

                if response.dragged_by(egui::PointerButton::Primary) {
                    if let Some((start, _)) = window.marquee {
                        if let Some(pos) = response.interact_pointer_pos() {
                            window.marquee = Some((start, pos));
                        }
                    } else if let Some(drag) = window.drag.as_mut() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let current_tick = tick_at(pos.x);
                            let current_pitch = pitch_at(pos.y);
                            match drag.mode {
                                NoteDragMode::Move => {
                                    let raw_delta =
                                        current_tick as i64 - drag.anchor_tick as i64;
                                    let anchor = drag
                                        .originals
                                        .first()
                                        .map(|(_, note)| note.start)
                                        .unwrap_or(0);
                                    let target_start = snap_tick(
                                        (anchor as i64 + raw_delta).max(0) as u64,
                                    );
                                    let delta_tick =
                                        target_start as i64 - anchor as i64;
                                    let delta_pitch = current_pitch as i16
                                        - drag.anchor_pitch as i16;
                                    for (index, original) in &drag.originals {
                                        if let Some(note) = window.notes.get_mut(*index) {
                                            let duration = original
                                                .end
                                                .saturating_sub(original.start)
                                                .max(1);
                                            let start = (original.start as i64
                                                + delta_tick)
                                                .clamp(
                                                    0,
                                                    length
                                                        .saturating_sub(duration)
                                                        as i64,
                                                )
                                                as u64;
                                            note.start = start;
                                            note.end = (start + duration)
                                                .min(length)
                                                .max(start + 1);
                                            note.pitch = (original.pitch as i16
                                                + delta_pitch)
                                                .clamp(0, 127)
                                                as u8;
                                        }
                                    }
                                    if let Some((index, _)) = drag.originals.first() {
                                        if let Some(note) = window.notes.get(*index) {
                                            if drag.preview_pitch != Some(note.pitch) {
                                                preview_note =
                                                    Some((note.pitch, note.velocity));
                                                drag.preview_pitch = Some(note.pitch);
                                            }
                                        }
                                    }
                                    commit = true;
                                }
                                NoteDragMode::ResizeEnd => {
                                    let delta =
                                        current_tick as i64 - drag.anchor_tick as i64;
                                    for (index, original) in &drag.originals {
                                        if let Some(note) = window.notes.get_mut(*index) {
                                            let end = snap_tick(
                                                (original.end as i64 + delta)
                                                    .max(original.start as i64 + 1)
                                                    as u64,
                                            )
                                            .min(length)
                                            .max(original.start + 1);
                                            note.end = end;
                                        }
                                    }
                                    if let Some((index, _)) = drag.originals.first() {
                                        if let Some(note) = window.notes.get(*index) {
                                            window.last_length =
                                                note.end.saturating_sub(note.start).max(1);
                                        }
                                    }
                                    commit = true;
                                }
                                NoteDragMode::Velocity => {
                                    let delta = (current_pitch as i16
                                        - drag.anchor_pitch as i16)
                                        * 4;
                                    for (index, original) in &drag.originals {
                                        if let Some(note) = window.notes.get_mut(*index) {
                                            note.velocity = (original.velocity as i16
                                                + delta)
                                                .clamp(1, 127)
                                                as u8;
                                        }
                                    }
                                    commit = true;
                                }
                            }
                        }
                    }
                }

                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    if let Some((start, end)) = window.marquee.take() {
                        let marquee =
                            egui::Rect::from_two_pos(start, end).intersect(grid);
                        let hits = notes_in_marquee(&note_rects, marquee);
                        if window.marquee_additive {
                            for index in hits {
                                window.selected.insert(index);
                            }
                        } else {
                            window.selected = hits;
                        }
                        window.marquee_additive = false;
                    } else if let Some(drag) = window.drag.take() {
                        let changed = drag.originals.iter().any(|(index, original)| {
                            window.notes.get(*index) != Some(original)
                        });
                        if changed {
                            undo_before = Some(drag.before);
                            commit = true;
                        }
                    }
                }

                if response.clicked() && !double_clicked && !handled_double {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if grid.contains(pos) {
                            match window.tool {
                                PianoRollTool::Erase => {
                                    if let Some((index, _)) = hit_test(pos) {
                                        undo_before = Some(clip.clone());
                                        window.notes.remove(index);
                                        window.selected.clear();
                                        window.drag = None;
                                        commit = true;
                                    }
                                }
                                PianoRollTool::Split => {
                                    if let Some((index, _)) = hit_test(pos) {
                                        let tick = snap_tick(tick_at(pos.x));
                                        let split = window.notes.get(index).and_then(
                                            |note| {
                                                (note.start < tick
                                                    && tick < note.end)
                                                    .then(|| PianoRollNote {
                                                        start: tick,
                                                        end: note.end,
                                                        pitch: note.pitch,
                                                        velocity: note.velocity,
                                                        channel: note.channel,
                                                    })
                                            },
                                        );
                                        if let Some(split) = split {
                                            undo_before = Some(clip.clone());
                                            window.notes[index].end = tick;
                                            window.notes.push(split);
                                            window.notes.sort_by_key(|note| {
                                                (
                                                    note.start,
                                                    note.pitch,
                                                    note.channel,
                                                )
                                            });
                                            window.selected.clear();
                                            window.drag = None;
                                            commit = true;
                                        }
                                    }
                                }
                                PianoRollTool::Glue => {
                                    if let Some((index, _)) = hit_test(pos) {
                                        let gap =
                                            if snap_ticks > 1 { snap_ticks } else { 0 };
                                        let before = clip.clone();
                                        if join_note_with_next(
                                            &mut window.notes,
                                            index,
                                            gap,
                                        ) {
                                            undo_before = Some(before);
                                            window.selected.clear();
                                            window.drag = None;
                                            commit = true;
                                        }
                                    }
                                }
                                PianoRollTool::Draw => {
                                    if let Some((index, _)) = hit_test(pos) {
                                        if modifiers.command || modifiers.shift {
                                            if window.selected.contains(&index) {
                                                window.selected.remove(&index);
                                            } else {
                                                window.selected.insert(index);
                                            }
                                        } else {
                                            window.selected.clear();
                                            window.selected.insert(index);
                                        }
                                        if let Some(note) = window.notes.get(index) {
                                            preview_note =
                                                Some((note.pitch, note.velocity));
                                        }
                                    } else if clip.length > 0 {
                                        let start = snap_tick(tick_at(pos.x))
                                            .min(length.saturating_sub(1));
                                        let wanted = piano_roll_new_note_length(
                                            window.snap,
                                            snap_ticks,
                                            window.last_length,
                                        );
                                        window.last_length = wanted;
                                        let end = start
                                            .saturating_add(wanted)
                                            .min(length)
                                            .max(start + 1);
                                        let note = PianoRollNote {
                                            start,
                                            end,
                                            pitch: pitch_at(pos.y),
                                            velocity: 100,
                                            channel: 0,
                                        };
                                        undo_before = Some(clip.clone());
                                        window.notes.push(note);
                                        let index = window.notes.len() - 1;
                                        window.selected.clear();
                                        window.selected.insert(index);
                                        preview_note = Some((
                                            window.notes[index].pitch,
                                            window.notes[index].velocity,
                                        ));
                                        commit = true;
                                    }
                                }
                                PianoRollTool::Select
                                | PianoRollTool::Velocity => {
                                    match hit_test(pos) {
                                        Some((index, _)) => {
                                            if modifiers.command || modifiers.shift {
                                                if window.selected.contains(&index) {
                                                    window.selected.remove(&index);
                                                } else {
                                                    window.selected.insert(index);
                                                }
                                            } else {
                                                window.selected.clear();
                                                window.selected.insert(index);
                                            }
                                            if let Some(note) = window.notes.get(index) {
                                                preview_note =
                                                    Some((note.pitch, note.velocity));
                                            }
                                        }
                                        None => {
                                            if !modifiers.command && !modifiers.shift {
                                                window.selected.clear();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((start, end)) = window.marquee {
                    let marquee =
                        egui::Rect::from_two_pos(start, end).intersect(grid);
                    painter.rect_filled(
                        marquee,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 40),
                    );
                    painter.rect_stroke(
                        marquee,
                        0.0,
                        egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(120, 170, 240),
                        ),
                    );
                }

                // Scrollbar tracks and thumbs.
                painter.rect_filled(h_scroll, 2.0, egui::Color32::from_gray(20));
                painter.rect_filled(v_scroll, 2.0, egui::Color32::from_gray(20));
                let h_thumb_width = if length == 0 {
                    grid.width()
                } else {
                    (grid.width() * (visible_ticks / length as f32))
                        .clamp(24.0_f32.min(grid.width()), grid.width())
                };
                let h_travel = (grid.width() - h_thumb_width).max(1.0);
                let h_fraction = if h_max > 0.0 {
                    (window.view_start_tick / h_max) as f32
                } else {
                    0.0
                };
                let h_thumb = egui::Rect::from_min_size(
                    egui::pos2(
                        grid.left() + h_fraction * h_travel,
                        h_scroll.top() + 1.0,
                    ),
                    egui::vec2(h_thumb_width, (h_scroll.height() - 2.0).max(1.0)),
                );
                painter.rect_filled(
                    h_thumb,
                    3.0,
                    if h_scroll_active {
                        egui::Color32::from_gray(150)
                    } else {
                        egui::Color32::from_gray(90)
                    },
                );

                let v_thumb_height = (grid.height()
                    * (visible_pitches / 128.0))
                    .clamp(24.0_f32.min(grid.height()), grid.height());
                let v_travel = (grid.height() - v_thumb_height).max(1.0);
                let v_fraction = if v_max > 0.0 {
                    (window.view_low_pitch / v_max) as f32
                } else {
                    0.0
                };
                let v_thumb = egui::Rect::from_min_size(
                    egui::pos2(
                        v_scroll.left() + 1.0,
                        grid.top() + (1.0 - v_fraction) * v_travel,
                    ),
                    egui::vec2(
                        (v_scroll.width() - 2.0).max(1.0),
                        v_thumb_height,
                    ),
                );
                painter.rect_filled(
                    v_thumb,
                    3.0,
                    if v_scroll_active {
                        egui::Color32::from_gray(150)
                    } else {
                        egui::Color32::from_gray(90)
                    },
                );
                if window.show_velocity_lane {
                    ui.add_space(6.0);
                    let (vel_response, vel_painter) = ui.allocate_painter(
                        egui::vec2(ui.available_width(), velocity_lane_height),
                        egui::Sense::click_and_drag(),
                    );
                let vel_rect = vel_response.rect;
                let vel_grid = egui::Rect::from_min_max(
                    egui::pos2(grid.left(), vel_rect.top()),
                    egui::pos2(grid.right(), vel_rect.bottom()),
                );
                vel_painter.rect_filled(vel_rect, 0.0, egui::Color32::from_gray(24));
                vel_painter.text(
                    egui::pos2(vel_rect.left() + 3.0, vel_rect.top() + 3.0),
                    egui::Align2::LEFT_TOP,
                    "Velocity",
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(180),
                );
                for (fraction, label) in
                    [(0.0_f32, "0"), (0.5, "64"), (1.0, "127")]
                {
                    let y = vel_grid.bottom() - fraction * vel_grid.height();
                    vel_painter.line_segment(
                        [
                            egui::pos2(vel_grid.left(), y),
                            egui::pos2(vel_grid.right(), y),
                        ],
                        egui::Stroke::new(0.5_f32, egui::Color32::from_gray(48)),
                    );
                    vel_painter.text(
                        egui::pos2(vel_grid.left() - 4.0, y),
                        egui::Align2::RIGHT_CENTER,
                        label,
                        egui::FontId::monospace(7.0),
                        egui::Color32::from_gray(130),
                    );
                }

                let mut velocity_bars: Vec<(usize, egui::Rect)> = Vec::new();
                for (index, note) in window.notes.iter().enumerate() {
                    let x = x_for_tick(note.start as f64);
                    if x < vel_grid.left() - 4.0 || x > vel_grid.right() + 4.0 {
                        continue;
                    }
                    let height = (note.velocity as f32 / 127.0)
                        * (vel_grid.height() - 6.0).max(1.0);
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(x - 2.5, vel_grid.bottom() - height),
                        egui::pos2(x + 2.5, vel_grid.bottom()),
                    );
                    let selected = window.selected.contains(&index);
                    let color = velocity_color(note.velocity);
                    let fill = if selected {
                        egui::Color32::from_rgb(
                            color.r().saturating_add(35),
                            color.g().saturating_add(35),
                            color.b().saturating_add(35),
                        )
                    } else {
                        color
                    };
                    vel_painter.rect_filled(bar, 1.0, fill);
                    if selected {
                        vel_painter.rect_stroke(
                            bar,
                            1.0,
                            egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
                        );
                    }
                    velocity_bars.push((index, bar));
                }

                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    if vel_response.hovered()
                        && velocity_bars
                            .iter()
                            .any(|(_, bar)| bar.expand(4.0).contains(pos))
                    {
                        ui.output_mut(|output| {
                            output.cursor_icon = egui::CursorIcon::ResizeVertical
                        });
                    }
                }

                if vel_response.drag_started_by(egui::PointerButton::Primary) {
                    window.velocity_drag_before = Some(clip.clone());
                }
                if vel_response.dragged_by(egui::PointerButton::Primary) {
                    if let Some(pos) = vel_response.interact_pointer_pos() {
                        if vel_grid.contains(pos) {
                            let mut nearest: Option<(usize, f32)> = None;
                            for (index, bar) in &velocity_bars {
                                let distance = (bar.center().x - pos.x).abs();
                                if distance <= 8.0
                                    && nearest.map_or(true, |(_, best)| distance < best)
                                {
                                    nearest = Some((*index, distance));
                                }
                            }
                            if let Some((index, _)) = nearest {
                                let targets: Vec<usize> =
                                    if window.selected.contains(&index) {
                                        window.selected.iter().copied().collect()
                                    } else {
                                        vec![index]
                                    };
                                let velocity = (((vel_grid.bottom() - pos.y)
                                    / vel_grid.height().max(1.0))
                                    * 127.0)
                                    .round()
                                    .clamp(1.0, 127.0)
                                    as u8;
                                for target in targets {
                                    if let Some(note) = window.notes.get_mut(target) {
                                        note.velocity = velocity;
                                    }
                                }
                                commit = true;
                            }
                        }
                    }
                }
                if vel_response.drag_stopped_by(egui::PointerButton::Primary) {
                    if let Some(before) = window.velocity_drag_before.take() {
                        undo_before = Some(before);
                        commit = true;
                    }
                }
                }
                let cc_lanes = window.cc_lanes.clone();
                for controller in cc_lanes {
                    ui.add_space(6.0);
                    let (cc_response, cc_painter) = ui.allocate_painter(
                        egui::vec2(ui.available_width(), cc_lane_height),
                        egui::Sense::click_and_drag(),
                    );
                    let cc_rect = cc_response.rect;
                    let cc_grid = egui::Rect::from_min_max(
                        egui::pos2(grid.left(), cc_rect.top()),
                        egui::pos2(grid.right(), cc_rect.bottom()),
                    );
                    let mut cc_points =
                        midi_clip_cc_points(&clip, controller);

                    if cc_response.drag_started_by(egui::PointerButton::Primary) {
                        window.cc_drag_before = Some(clip.clone());
                    }
                    if cc_response.dragged_by(egui::PointerButton::Primary) {
                        if let Some(pos) = cc_response.interact_pointer_pos() {
                            if cc_grid.contains(pos) {
                                let tick = snap_tick(tick_at(pos.x));
                                let value = (((cc_grid.bottom() - pos.y)
                                    / cc_grid.height().max(1.0))
                                    * 127.0)
                                    .round()
                                    .clamp(0.0, 127.0)
                                    as u8;
                                cc_points.retain(|point| point.tick != tick);
                                cc_points.push(MidiCcPoint {
                                    tick,
                                    value,
                                    channel: 0,
                                });
                                cc_points.sort_by_key(|point| point.tick);
                                if let Some(target) =
                                    self.project.midi_clips.get_mut(window.clip_id)
                                {
                                    write_midi_cc_points(
                                        target,
                                        controller,
                                        &cc_points,
                                    );
                                }
                            }
                        }
                    }
                    if cc_response.drag_stopped_by(egui::PointerButton::Primary) {
                        cc_undo_before = window.cc_drag_before.take();
                    }

                    cc_painter.rect_filled(cc_rect, 0.0, egui::Color32::from_gray(24));
                    cc_painter.text(
                        egui::pos2(cc_rect.left() + 3.0, cc_rect.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        format!(
                            "CC {} · {}",
                            controller,
                            cc_controller_label(controller)
                        ),
                        egui::FontId::proportional(9.0),
                        egui::Color32::from_gray(180),
                    );
                    for (fraction, label) in
                        [(0.0_f32, "0"), (0.5, "64"), (1.0, "127")]
                    {
                        let y = cc_grid.bottom() - fraction * cc_grid.height();
                        cc_painter.line_segment(
                            [
                                egui::pos2(cc_grid.left(), y),
                                egui::pos2(cc_grid.right(), y),
                            ],
                            egui::Stroke::new(0.5_f32, egui::Color32::from_gray(48)),
                        );
                        cc_painter.text(
                            egui::pos2(cc_grid.left() - 4.0, y),
                            egui::Align2::RIGHT_CENTER,
                            label,
                            egui::FontId::monospace(7.0),
                            egui::Color32::from_gray(130),
                        );
                    }

                    let mut screen_points = Vec::new();
                    for point in &cc_points {
                        let x = x_for_tick(point.tick as f64);
                        if x < cc_grid.left() - 4.0 || x > cc_grid.right() + 4.0 {
                            continue;
                        }
                        let y = cc_grid.bottom()
                            - (point.value as f32 / 127.0)
                                * (cc_grid.height() - 6.0).max(1.0);
                        screen_points.push(egui::pos2(x, y));
                    }
                    if screen_points.len() > 1 {
                        cc_painter.add(egui::Shape::line(
                            screen_points.clone(),
                            egui::Stroke::new(
                                1.5_f32,
                                egui::Color32::from_rgb(90, 200, 255),
                            ),
                        ));
                    }
                    for point in &screen_points {
                        cc_painter.circle_filled(
                            *point,
                            2.5,
                            egui::Color32::from_rgb(150, 230, 255),
                        );
                    }

                    if cc_response.hovered() {
                        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                            if cc_grid.contains(pos) {
                                ui.output_mut(|output| {
                                    output.cursor_icon = egui::CursorIcon::Crosshair
                                });
                            }
                        }
                    }
                }
                    });
                });
            });

        if !open || close {
            if commit {
                self.write_piano_roll_to_project(&window);
            }
            let before = undo_before.or_else(|| window.drag.take().map(|drag| drag.before));
            if let Some(before) = before {
                self.push_midi_clip_undo(window.clip_id, before);
            }
            self.stop_piano_roll_preview();
            self.piano_roll = None;
            return;
        }

        if let Some(tick) = seek_content_tick {
            let timeline_tick = (tick as i64
                + clip.timeline_position.tick as i64
                - clip.content_offset as i64)
                .max(0) as u64;
            let sample = self
                .project
                .tempo_map
                .tick_to_sample(timeline_tick, sample_rate);
            if self.playing || self.recording {
                self.pause_transport();
            }
            self.playhead_secs = sample as f64 / sample_rate;
        }
        if commit {
            self.write_piano_roll_to_project(&window);
        }
        if let Some(before) = undo_before {
            self.push_midi_clip_undo(window.clip_id, before);
        }
        if let Some(before) = cc_undo_before {
            self.push_midi_clip_undo(window.clip_id, before);
        }
        if do_quantize {
            let grid = window.tools.quantize_snap.ticks(beats_per_bar);
            let strength = window.tools.quantize_strength;
            let swing = window.tools.quantize_swing / 100.0;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Quantize MIDI",
                |notes| quantize_midi_notes(notes, grid, strength, swing),
            );
            window.initialized = false;
        }
        if do_transpose {
            let semitones = window.tools.transpose;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Transpose MIDI",
                |notes| transpose_midi_notes(notes, semitones),
            );
            window.initialized = false;
        }
        if do_scale {
            let root = window.tools.scale_root;
            let intervals = window.tools.scale_type.intervals();
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Conform MIDI to scale",
                |notes| scale_conform_midi_notes(notes, root, intervals),
            );
            window.initialized = false;
        }
        if do_velocity {
            let mode = window.tools.velocity_mode;
            let factor = window.tools.velocity_factor;
            let amount = (window.tools.velocity_amount / 100.0).clamp(0.0, 1.0);
            let curve = (window.tools.velocity_curve / 100.0).clamp(-1.0, 1.0);
            let random = window.tools.velocity_random.round().clamp(0.0, 64.0) as u8;
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(1);
            let label = match mode {
                VelocityMode::Scale => "Scale MIDI velocity",
                VelocityMode::Compress => "Compress MIDI velocity",
                VelocityMode::Expand => "Expand MIDI velocity",
                VelocityMode::Curve => "Curve MIDI velocity",
                VelocityMode::Randomize => "Randomize MIDI velocity",
            };
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                label,
                |notes| match mode {
                    VelocityMode::Scale => scale_midi_velocities(notes, factor),
                    VelocityMode::Compress => {
                        compress_midi_velocities(notes, amount)
                    }
                    VelocityMode::Expand => {
                        expand_midi_velocities(notes, amount)
                    }
                    VelocityMode::Curve => curve_midi_velocities(notes, curve),
                    VelocityMode::Randomize => {
                        randomize_midi_notes(notes, 0, random, 0, seed)
                    }
                },
            );
            window.initialized = false;
        }
        if do_humanize {
            let timing = window.tools.humanize_timing.round().max(0.0) as u64;
            let velocity = window.tools.humanize_velocity.round().clamp(0.0, 127.0) as u8;
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(1);
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Humanize MIDI",
                |notes| humanize_midi_notes(notes, timing, velocity, seed),
            );
            window.initialized = false;
        }
        if do_legato {
            let gap = window.tools.legato_gap.round().max(0.0) as u64;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Legato MIDI",
                |notes| legato_midi_notes(notes, gap),
            );
            window.initialized = false;
        }
        if do_note_length {
            let length = window.tools.note_length.round().max(1.0) as u64;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Set MIDI note length",
                |notes| set_midi_note_length(notes, length),
            );
            window.initialized = false;
        }
        if do_join {
            let gap = window.tools.join_gap.round().clamp(0.0, 960.0) as u64;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Join MIDI notes",
                |notes| join_midi_notes(notes, gap),
            );
            window.initialized = false;
        }
        if do_split {
            if let Some(tick) = playhead_content_tick {
                self.apply_midi_note_edit_to_clips(
                    vec![window.clip_id],
                    "Split MIDI notes",
                    |notes| split_midi_notes_at_tick(notes, tick),
                );
                window.initialized = false;
            }
        }
        if do_reverse {
            let span = length;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Reverse MIDI",
                |notes| reverse_midi_notes(notes, span),
            );
            window.initialized = false;
        }
        if do_mirror {
            let center = window.tools.mirror_center;
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Mirror MIDI pitch",
                |notes| mirror_midi_notes(notes, center),
            );
            window.initialized = false;
        }
        if do_randomize {
            let pitch = window.tools.randomize_pitch.round().clamp(0.0, 12.0) as u8;
            let velocity = window.tools.randomize_velocity.round().clamp(0.0, 64.0) as u8;
            let timing = window.tools.randomize_timing.round().clamp(0.0, 240.0) as u64;
            let keep = 1.0 - window.tools.randomize_density.clamp(0.0, 100.0) / 100.0;
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(1);
            self.apply_midi_note_edit_to_clips(
                vec![window.clip_id],
                "Randomize MIDI",
                |notes| {
                    randomize_midi_notes(notes, pitch, velocity, timing, seed);
                    thin_midi_notes(
                        notes,
                        keep,
                        seed.wrapping_add(0x9e37_79b9_7f4a_7c15),
                    );
                },
            );
            window.initialized = false;
        }
        if let Some((pitch, velocity)) = preview_note {
            self.preview_piano_roll_note(pitch, velocity);
        }
        self.piano_roll = Some(window);
    }
}

/// Clamp a fade length to a clip's length.
fn clamp_fade(fade: Option<Fade>, clip_length: u64) -> Option<Fade> {
    fade.map(|fade| Fade::new(fade.length.min(clip_length), fade.curve))
}

/// Build a fade from a dragged length, keeping the existing curve. Very short
/// lengths remove the fade entirely.
fn make_fade(length: u64, existing: Option<Fade>, clip_length: u64) -> Option<Fade> {
    const MIN_FADE_SAMPLES: u64 = 16;
    if length < MIN_FADE_SAMPLES {
        return None;
    }
    let curve = existing.map(|fade| fade.curve).unwrap_or(FadeCurve::Linear);
    Some(Fade::new(length.min(clip_length), curve))
}

const KNOB_WIDTH: f32 = 82.0;
const KNOB_HEIGHT: f32 = 84.0;
const KNOB_CENTER_Y: f32 = 43.0;
const KNOB_RADIUS: f32 = 17.0;
const KNOB_ARC_RADIUS: f32 = 22.0;
const KNOB_START_ANGLE: f32 = std::f32::consts::PI * 0.75;
const KNOB_SWEEP: f32 = std::f32::consts::PI * 1.5;
const KNOB_DRAG_PIXELS: f32 = 160.0;

/// Convert a value to a 0..1 knob position, optionally using a logarithmic
/// taper (used for frequencies).
fn knob_to_normalized(value: f32, min: f32, max: f32, logarithmic: bool) -> f32 {
    if max <= min {
        return 0.0;
    }
    if logarithmic && min > 0.0 && max > 0.0 {
        ((value.max(min) / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    }
}

fn knob_from_normalized(norm: f32, min: f32, max: f32, logarithmic: bool) -> f32 {
    let norm = norm.clamp(0.0, 1.0);
    if max <= min {
        min
    } else if logarithmic && min > 0.0 && max > 0.0 {
        min * (max / min).powf(norm)
    } else {
        min + (max - min) * norm
    }
}

fn paint_knob_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    start: f32,
    end: f32,
    stroke: egui::Stroke,
) {
    let steps = (((end - start).abs() / 0.08).ceil() as usize).max(2);
    let points = (0..=steps)
        .map(|index| {
            let t = index as f32 / steps as f32;
            let angle = start + (end - start) * t;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
}

/// Mouse-wheel adjustment for knobs, matching sliders at 5% per notch.
fn apply_knob_wheel(
    ui: &mut egui::Ui,
    response: &egui::Response,
    norm: &mut f32,
) {
    if !response.hovered() {
        return;
    }
    let steps = slider_wheel_steps(ui);
    if steps == 0.0 {
        return;
    }
    ui.input_mut(|i| {
        i.raw_scroll_delta = egui::Vec2::ZERO;
        i.smooth_scroll_delta = egui::Vec2::ZERO;
    });
    let now = ui.input(|i| i.time);
    let last_id = response.id.with("knob_wheel_time");
    let last = ui
        .data(|data| data.get_temp::<f64>(last_id))
        .unwrap_or(f64::NEG_INFINITY);
    if now - last < 0.05 {
        return;
    }
    ui.data_mut(|data| data.insert_temp(last_id, now));
    *norm = (*norm + steps * 0.05).clamp(0.0, 1.0);
}

/// Logic-style rotary control. Drag vertically to change the value, hold Shift
/// for fine adjustment, and double-click to return to the default.
fn knob(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    label: &str,
    logarithmic: bool,
    format: impl Fn(f32) -> String,
) -> egui::Response {
    let min = *range.start();
    let max = *range.end();
    let default_norm = knob_to_normalized(default, min, max, logarithmic);
    let mut norm = knob_to_normalized(*value, min, max, logarithmic);

    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(KNOB_WIDTH, KNOB_HEIGHT),
        egui::Sense::click_and_drag(),
    );

    if response.double_clicked() {
        norm = default_norm;
    } else if response.dragged_by(egui::PointerButton::Primary) {
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.1 } else { 1.0 };
        norm -= response.drag_delta().y / KNOB_DRAG_PIXELS * speed;
        norm = norm.clamp(0.0, 1.0);
    }
    apply_knob_wheel(ui, &response, &mut norm);

    let keyboard_step = if ui.input(|input| input.modifiers.shift) {
        0.005
    } else {
        0.02
    };
    let keyboard_delta = ui.input(|input| {
        let mut delta = 0.0;
        if input.key_pressed(egui::Key::ArrowUp)
            || input.key_pressed(egui::Key::ArrowRight)
        {
            delta += keyboard_step;
        }
        if input.key_pressed(egui::Key::ArrowDown)
            || input.key_pressed(egui::Key::ArrowLeft)
        {
            delta -= keyboard_step;
        }
        delta
    });
    if response.has_focus() && keyboard_delta != 0.0 {
        norm = (norm + keyboard_delta).clamp(0.0, 1.0);
    }

    let old_value = *value;
    let next = knob_from_normalized(norm, min, max, logarithmic).clamp(min, max);
    if next != old_value {
        *value = next;
        response.mark_changed();
    }

    if response.clicked() {
        response.request_focus();
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let value_text = format(*value);
    let center = egui::pos2(rect.center().x, rect.top() + KNOB_CENTER_Y);
    let visuals = ui.style().interact(&response).clone();
    let painter = ui.painter();
    let accent = ui.visuals().selection.bg_fill;
    let track = ui.visuals().widgets.inactive.bg_stroke.color;
    let tick_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text_color = ui.visuals().text_color();
    let weak_text = ui.visuals().weak_text_color();

    painter.text(
        egui::pos2(rect.center().x, rect.top()),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::proportional(10.0),
        weak_text,
    );

    for tick in 0..=4 {
        let angle = KNOB_START_ANGLE + KNOB_SWEEP * tick as f32 / 4.0;
        let direction = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment(
            [
                center + direction * (KNOB_ARC_RADIUS + 3.0),
                center + direction * (KNOB_ARC_RADIUS + 5.0),
            ],
            egui::Stroke::new(1.0_f32, tick_color),
        );
    }

    paint_knob_arc(
        painter,
        center,
        KNOB_ARC_RADIUS,
        KNOB_START_ANGLE,
        KNOB_START_ANGLE + KNOB_SWEEP,
        egui::Stroke::new(2.0_f32, track),
    );
    // Bipolar ranges (for example pitch) fill outwards from the centre.
    let arc_origin = if min < 0.0 && max > 0.0 {
        knob_to_normalized(0.0, min, max, logarithmic)
    } else {
        0.0
    };
    let (fill_start, fill_end) = if norm >= arc_origin {
        (
            KNOB_START_ANGLE + KNOB_SWEEP * arc_origin,
            KNOB_START_ANGLE + KNOB_SWEEP * norm,
        )
    } else {
        (
            KNOB_START_ANGLE + KNOB_SWEEP * norm,
            KNOB_START_ANGLE + KNOB_SWEEP * arc_origin,
        )
    };
    if (fill_end - fill_start).abs() > 0.001 {
        paint_knob_arc(
            painter,
            center,
            KNOB_ARC_RADIUS,
            fill_start,
            fill_end,
            egui::Stroke::new(2.5_f32, accent),
        );
    }

    painter.circle_filled(center, KNOB_RADIUS, visuals.bg_fill);
    painter.circle_stroke(center, KNOB_RADIUS, visuals.bg_stroke);
    painter.circle_stroke(
        center,
        KNOB_RADIUS - 3.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(8)),
    );

    let pointer_angle = KNOB_START_ANGLE + KNOB_SWEEP * norm;
    let pointer_direction = egui::vec2(pointer_angle.cos(), pointer_angle.sin());
    painter.line_segment(
        [
            center + pointer_direction * (KNOB_RADIUS * 0.35),
            center + pointer_direction * (KNOB_RADIUS * 0.86),
        ],
        egui::Stroke::new(2.0_f32, visuals.fg_stroke.color),
    );
    painter.circle_filled(center, 2.0, accent);

    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 1.0),
        egui::Align2::CENTER_BOTTOM,
        &value_text,
        egui::FontId::proportional(11.0),
        text_color,
    );

    response.on_hover_text(format!(
        "{}: {}\nDrag vertically to adjust · Shift for fine · Double-click to reset",
        label, value_text
    ))
}

/// Compact rotary control used in dense rows such as the track list. The
/// label and value are supplied by the caller.
fn compact_knob(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    logarithmic: bool,
) -> egui::Response {
    let min = *range.start();
    let max = *range.end();
    let default_norm = knob_to_normalized(default, min, max, logarithmic);
    let mut norm = knob_to_normalized(*value, min, max, logarithmic);
    let size = 26.0;

    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(size, size),
        egui::Sense::click_and_drag(),
    );

    if response.double_clicked() {
        norm = default_norm;
    } else if response.dragged_by(egui::PointerButton::Primary) {
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.1 } else { 1.0 };
        norm -= response.drag_delta().y / 120.0 * speed;
        norm = norm.clamp(0.0, 1.0);
    }
    apply_knob_wheel(ui, &response, &mut norm);

    let old_value = *value;
    let next = knob_from_normalized(norm, min, max, logarithmic).clamp(min, max);
    if next != old_value {
        *value = next;
        response.mark_changed();
    }
    if response.clicked() {
        response.request_focus();
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let center = rect.center();
    let radius = size * 0.30;
    let arc_radius = size * 0.44;
    let visuals = ui.style().interact(&response).clone();
    let accent = ui.visuals().selection.bg_fill;
    let track = ui.visuals().widgets.inactive.bg_stroke.color;
    let painter = ui.painter();

    paint_knob_arc(
        painter,
        center,
        arc_radius,
        KNOB_START_ANGLE,
        KNOB_START_ANGLE + KNOB_SWEEP,
        egui::Stroke::new(1.5_f32, track),
    );
    let arc_origin = if min < 0.0 && max > 0.0 {
        knob_to_normalized(0.0, min, max, logarithmic)
    } else {
        0.0
    };
    let (fill_start, fill_end) = if norm >= arc_origin {
        (
            KNOB_START_ANGLE + KNOB_SWEEP * arc_origin,
            KNOB_START_ANGLE + KNOB_SWEEP * norm,
        )
    } else {
        (
            KNOB_START_ANGLE + KNOB_SWEEP * norm,
            KNOB_START_ANGLE + KNOB_SWEEP * arc_origin,
        )
    };
    if (fill_end - fill_start).abs() > 0.001 {
        paint_knob_arc(
            painter,
            center,
            arc_radius,
            fill_start,
            fill_end,
            egui::Stroke::new(2.0_f32, accent),
        );
    }
    painter.circle_filled(center, radius, visuals.bg_fill);
    painter.circle_stroke(center, radius, visuals.bg_stroke);

    let pointer_angle = KNOB_START_ANGLE + KNOB_SWEEP * norm;
    let pointer_direction = egui::vec2(pointer_angle.cos(), pointer_angle.sin());
    painter.line_segment(
        [
            center + pointer_direction * (radius * 0.30),
            center + pointer_direction * (radius * 0.86),
        ],
        egui::Stroke::new(1.5_f32, visuals.fg_stroke.color),
    );
    painter.circle_filled(center, 1.5, accent);

    response.on_hover_text(
        "Drag vertically to adjust · Shift for fine · Double-click to reset",
    )
}

/// Compact knob with a small label above and value below.
fn compact_knob_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    logarithmic: bool,
    format: impl Fn(f32) -> String,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small());
        let response = compact_knob(ui, value, range, default, logarithmic);
        ui.label(egui::RichText::new(format(*value)).small());
        response
    })
    .inner
}

fn format_frequency(value: f32) -> String {
    if value >= 1_000.0 {
        format!("{:.1} kHz", value / 1_000.0)
    } else {
        format!("{:.0} Hz", value)
    }
}

fn stretch_mode_label(mode: TimeStretchMode) -> &'static str {
    match mode {
        TimeStretchMode::Tape => "Tape",
        TimeStretchMode::Elastic => "Elastic",
        TimeStretchMode::Musical => "Musical",
    }
}

fn filter_mode_label(mode: FilterMode) -> &'static str {
    match mode {
        FilterMode::LowPass => "Low-pass",
        FilterMode::HighPass => "High-pass",
    }
}

fn track_type_label(track_type: TrackType) -> &'static str {
    match track_type {
        TrackType::Audio => "Audio",
        TrackType::Midi => "MIDI",
        TrackType::Instrument => "Instrument",
        TrackType::Bus => "Bus",
        TrackType::Folder => "Folder",
    }
}

/// Render one track's insert chain. Used by the FX Rack, where every track is
/// a column and the slot controls need to stay compact.
fn show_insert_slots(ui: &mut egui::Ui, inserts: &mut Vec<InsertSlot>) -> bool {
    let mut changed = false;
    let mut remove = None;
    let mut move_up = None;
    let mut move_down = None;
    let slot_count = inserts.len();

    if inserts.is_empty() {
        ui.weak("No inserts");
    }

    for (index, slot) in inserts.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}", index + 1)).strong(),
                    );
                    let current = slot
                        .effect
                        .as_ref()
                        .map(InsertEffectKind::from_effect);
                    let mut selected = current;
                    egui::ComboBox::from_id_salt("kind")
                        .selected_text(
                            current.map_or("Empty", InsertEffectKind::label),
                        )
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            for kind in InsertEffectKind::ALL {
                                ui.selectable_value(
                                    &mut selected,
                                    Some(kind),
                                    kind.label(),
                                );
                            }
                        });
                    if selected != current {
                        slot.effect =
                            selected.map(InsertEffectKind::default_effect);
                        changed = true;
                    }
                    changed |= ui.checkbox(&mut slot.enabled, "On").changed();
                });

                ui.horizontal(|ui| {
                    if ui
                        .small_button("↑")
                        .on_hover_text("Move effect up")
                        .clicked()
                        && index > 0
                    {
                        move_up = Some(index);
                    }
                    if ui
                        .small_button("↓")
                        .on_hover_text("Move effect down")
                        .clicked()
                        && index + 1 < slot_count
                    {
                        move_down = Some(index);
                    }
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove effect")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });

                match &mut slot.effect {
                    Some(TrackEffect::Saturation { drive_db, mix }) => {
                        ui.horizontal_top(|ui| {
                            changed |= knob(
                                ui,
                                drive_db,
                                0.0..=36.0,
                                12.0,
                                "Drive",
                                false,
                                |value| format!("{:.1} dB", value),
                            )
                            .changed();
                            changed |= knob(
                                ui,
                                mix,
                                0.0..=1.0,
                                1.0,
                                "Mix",
                                false,
                                |value| format!("{:.0}%", value * 100.0),
                            )
                            .changed();
                        });
                    }
                    Some(TrackEffect::Filter { mode, cutoff }) => {
                        ui.horizontal(|ui| {
                            ui.label("Type:");
                            egui::ComboBox::from_id_salt("filter_mode")
                                .selected_text(filter_mode_label(*mode))
                                .width(104.0)
                                .show_ui(ui, |ui| {
                                    changed |= ui
                                        .selectable_value(
                                            mode,
                                            FilterMode::LowPass,
                                            filter_mode_label(FilterMode::LowPass),
                                        )
                                        .changed();
                                    changed |= ui
                                        .selectable_value(
                                            mode,
                                            FilterMode::HighPass,
                                            filter_mode_label(FilterMode::HighPass),
                                        )
                                        .changed();
                                });
                        });
                        changed |= knob(
                            ui,
                            cutoff,
                            20.0..=20_000.0,
                            8_000.0,
                            "Cutoff",
                            true,
                            format_frequency,
                        )
                        .changed();
                    }
                    Some(TrackEffect::BitCrusher {
                        bits,
                        downsample,
                    }) => {
                        ui.horizontal_top(|ui| {
                            let mut bit_value = *bits as f32;
                            if knob(
                                ui,
                                &mut bit_value,
                                1.0..=24.0,
                                8.0,
                                "Bits",
                                false,
                                |value| format!("{:.0}", value),
                            )
                            .changed()
                            {
                                *bits = bit_value.round() as u32;
                                changed = true;
                            }
                            let mut hold_value = *downsample as f32;
                            if knob(
                                ui,
                                &mut hold_value,
                                1.0..=64.0,
                                4.0,
                                "Hold",
                                false,
                                |value| format!("{:.0}×", value),
                            )
                            .changed()
                            {
                                *downsample = hold_value.round() as u32;
                                changed = true;
                            }
                        });
                    }
                    Some(TrackEffect::Tremolo { rate, depth }) => {
                        ui.horizontal_top(|ui| {
                            changed |= knob(
                                ui,
                                rate,
                                0.1..=20.0,
                                5.0,
                                "Rate",
                                true,
                                format_frequency,
                            )
                            .changed();
                            changed |= knob(
                                ui,
                                depth,
                                0.0..=1.0,
                                0.5,
                                "Depth",
                                false,
                                |value| format!("{:.0}%", value * 100.0),
                            )
                            .changed();
                        });
                    }
                    None => {
                        ui.weak("Choose an effect kind above.");
                    }
                }

                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    changed |= compact_knob_control(
                        ui,
                        "In",
                        &mut slot.input_gain,
                        0.0..=2.0,
                        1.0,
                        false,
                        |value| format!("{:.2}×", value),
                    )
                    .changed();
                    changed |= compact_knob_control(
                        ui,
                        "Mix",
                        &mut slot.mix,
                        0.0..=1.0,
                        1.0,
                        false,
                        |value| format!("{:.2}", value),
                    )
                    .changed();
                    changed |= compact_knob_control(
                        ui,
                        "Out",
                        &mut slot.output_gain,
                        0.0..=2.0,
                        1.0,
                        false,
                        |value| format!("{:.2}×", value),
                    )
                    .changed();
                });
            });
        });
        ui.add_space(4.0);
    }

    if let Some(index) = move_up {
        inserts.swap(index, index - 1);
        changed = true;
    }
    if let Some(index) = move_down {
        inserts.swap(index, index + 1);
        changed = true;
    }
    if let Some(index) = remove {
        inserts.remove(index);
        changed = true;
    }

    ui.menu_button("+ Add Effect", |ui| {
        for kind in InsertEffectKind::ALL {
            if ui.button(kind.label()).clicked() {
                inserts.push(InsertSlot::with_effect(kind.default_effect()));
                changed = true;
                ui.close_menu();
            }
        }
    });
    if !inserts.is_empty() && ui.small_button("Clear All").clicked() {
        inserts.clear();
        changed = true;
    }
    changed
}

#[derive(Debug, Clone, Copy)]
struct LoudnessMeasurement {
    integrated_lufs: f32,
    peak: f32,
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// ITU-R BS.1770 K-weighting high shelf (stage 1). Uses the same prewarped
/// analog prototype as libebur128 so the 48 kHz coefficients match the
/// published values exactly.
fn k_weighting_shelf(sample_rate: f64) -> Biquad {
    const FREQ: f64 = 1_681.974_450_955_533;
    const GAIN_DB: f64 = 3.999_843_853_973_347;
    const Q: f64 = 0.707_175_236_955_419_6;

    let sample_rate = sample_rate.max(1.0);
    let freq = FREQ.min(sample_rate * 0.49);
    let k = (std::f64::consts::PI * freq / sample_rate).tan();
    let vh = 10.0_f64.powf(GAIN_DB / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / Q + k * k;

    Biquad {
        b0: (vh + vb * k / Q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / Q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / Q + k * k) / a0,
        z1: 0.0,
        z2: 0.0,
    }
}

/// ITU-R BS.1770 K-weighting high-pass (stage 2).
fn k_weighting_high_pass(sample_rate: f64) -> Biquad {
    const FREQ: f64 = 38.135_470_876_024_44;
    const Q: f64 = 0.500_327_037_323_877_3;

    let sample_rate = sample_rate.max(1.0);
    let freq = FREQ.min(sample_rate * 0.49);
    let k = (std::f64::consts::PI * freq / sample_rate).tan();
    let a0 = 1.0 + k / Q + k * k;

    Biquad {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / Q + k * k) / a0,
        z1: 0.0,
        z2: 0.0,
    }
}

fn loudness_from_energy(energy: f64) -> f64 {
    if energy <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * energy.log10()
    }
}

/// Integrated loudness in LUFS using the BS.1770 K-weighting filter, 400 ms
/// blocks with 100 ms hops, and the −70 LUFS absolute / −10 LU relative gates.
/// Peak is the raw sample peak of the analysed range.
fn measure_integrated_loudness(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Option<LoudnessMeasurement> {
    let channel_count = channels.max(1) as usize;
    let frames = samples.len() / channel_count;
    if frames == 0 {
        return None;
    }

    let sample_rate = sample_rate.max(1) as f64;
    let block_frames = ((sample_rate * 0.4).round() as usize).clamp(1, frames);
    let hop_frames = ((sample_rate * 0.1).round() as usize).clamp(1, block_frames);
    let mut filters = vec![
        (
            k_weighting_shelf(sample_rate),
            k_weighting_high_pass(sample_rate),
        );
        channel_count
    ];

    let mut ring = vec![0.0_f64; block_frames];
    let mut ring_pos = 0usize;
    let mut energy_sum = 0.0_f64;
    let mut blocks = Vec::new();
    let mut peak = 0.0f32;

    for frame in 0..frames {
        let base = frame * channel_count;
        let mut frame_energy = 0.0_f64;
        for (channel, (shelf, high_pass)) in filters.iter_mut().enumerate() {
            let sample = samples[base + channel];
            peak = peak.max(sample.abs());
            let filtered = high_pass.process(shelf.process(sample as f64));
            frame_energy += filtered * filtered;
        }

        if frame >= block_frames {
            energy_sum -= ring[ring_pos];
        }
        ring[ring_pos] = frame_energy;
        energy_sum += frame_energy;
        ring_pos += 1;
        if ring_pos == block_frames {
            ring_pos = 0;
        }

        let frames_seen = frame + 1;
        if frames_seen >= block_frames && (frames_seen - block_frames) % hop_frames == 0 {
            blocks.push(energy_sum / block_frames as f64);
        }
    }

    let mut absolute_sum = 0.0;
    let mut absolute_count = 0usize;
    for &energy in &blocks {
        if loudness_from_energy(energy) > -70.0 {
            absolute_sum += energy;
            absolute_count += 1;
        }
    }
    if absolute_count == 0 {
        return None;
    }

    let ungated_loudness = loudness_from_energy(absolute_sum / absolute_count as f64);
    let relative_gate = ungated_loudness - 10.0;
    let mut relative_sum = 0.0;
    let mut relative_count = 0usize;
    for &energy in &blocks {
        let loudness = loudness_from_energy(energy);
        if loudness > -70.0 && loudness > relative_gate {
            relative_sum += energy;
            relative_count += 1;
        }
    }
    if relative_count == 0 {
        return None;
    }

    let integrated_lufs = loudness_from_energy(relative_sum / relative_count as f64);
    if !integrated_lufs.is_finite() {
        return None;
    }

    Some(LoudnessMeasurement {
        integrated_lufs: integrated_lufs as f32,
        peak,
    })
}

/// Clip gain at a source-frame offset after applying the fade envelopes.
/// Mirrors the playback mixer's gain calculation.
fn clip_gain_at(
    frame: u64,
    clip_frames: u64,
    gain: f32,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
) -> f32 {
    let mut value = gain;
    if let Some(fade) = fade_in {
        if frame < fade.length {
            value *= fade.gain_at(frame);
        }
    }
    if let Some(fade) = fade_out {
        if fade.length > 0 {
            let start = clip_frames.saturating_sub(fade.length);
            if frame >= start {
                let t = ((frame - start) as f32 / fade.length as f32).clamp(0.0, 1.0);
                value *= 1.0 - fade.curve.apply(t);
            }
        }
    }
    value.clamp(0.0, 4.0)
}

fn fade_curve_name(curve: FadeCurve) -> &'static str {
    match curve {
        FadeCurve::Linear => "Linear",
        FadeCurve::Exponential => "Exponential",
        FadeCurve::Logarithmic => "Logarithmic",
        FadeCurve::SCurve => "S-Curve",
        FadeCurve::EqualPower => "Equal power",
    }
}

/// Compact fade editor: curve selector, length in seconds, and an "off" entry.
/// Returns the new fade and whether a discrete (non-drag) change happened.
fn fade_editor_ui(
    ui: &mut egui::Ui,
    label: &str,
    fade: Option<Fade>,
    clip_length: u64,
    source_sr: f64,
) -> (Option<Fade>, bool) {
    let mut result = fade;
    let mut discrete = false;

    ui.label(label);
    let selected_text = fade
        .map(|fade| fade_curve_name(fade.curve))
        .unwrap_or("off");
    egui::ComboBox::from_id_salt(("fade_curve", label))
        .selected_text(selected_text)
        .width(92.0)
        .show_ui(ui, |ui| {
            for curve in [
                FadeCurve::Linear,
                FadeCurve::Exponential,
                FadeCurve::Logarithmic,
                FadeCurve::SCurve,
                FadeCurve::EqualPower,
            ] {
                if ui
                    .selectable_label(fade.map(|f| f.curve) == Some(curve), fade_curve_name(curve))
                    .clicked()
                {
                    let length = fade
                        .map(|f| f.length)
                        .unwrap_or_else(|| ((clip_length as f64 * 0.1) as u64).max(1));
                    result = Some(Fade::new(length.min(clip_length), curve));
                    discrete = true;
                }
            }
            if ui.selectable_label(fade.is_none(), "off").clicked() {
                result = None;
                discrete = true;
            }
        });

    if let Some(current) = fade {
        let max_secs = clip_length as f64 / source_sr.max(1.0);
        let mut secs = current.length as f64 / source_sr.max(1.0);
        if ui
            .add(
                egui::DragValue::new(&mut secs)
                    .speed(0.005)
                    .range(0.0..=max_secs)
                    .suffix(" s"),
            )
            .changed()
        {
            let length = (secs * source_sr).round() as u64;
            result = make_fade(length, Some(current), clip_length);
        }
    }

    (result, discrete)
}

impl eframe::App for AppUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Hold Alt to bypass snapping while dragging clips.
        self.snap_suspended = ctx.input(|i| i.modifiers.alt);
        // A released pointer ends the current undo-merge gesture.
        if !ctx.input(|i| i.pointer.any_down()) {
            self.merge_chain = false;
            self.commit_pending_shape_edit();
            self.commit_pending_insert_edit();
            if self.needs_audio_refresh {
                self.needs_audio_refresh = false;
                self.refresh_playback_if_playing();
            }
        }

        // Handle keyboard shortcuts
        self.handle_keyboard(ctx);

        // Pick up finished waveform overviews
        self.poll_waveforms();

        // Apply finished background playback reloads
        self.poll_playback_reload();

        // Advance the playhead during playback. Prefer the audio engine's
        // sample clock so the playhead matches what is actually being heard,
        // and fall back to wall time when no audio is loaded. Playback is not
        // clamped to the last clip: the playhead may run past the project end.
        if self.playing {
            if let Some(frames) = self.engine.playback_position_frames() {
                let rate = self.engine.output_sample_rate().max(1) as f64;
                self.playhead_secs = frames as f64 / rate;
            } else {
                let elapsed = self.last_playhead_update.elapsed().as_secs_f64();
                self.playhead_secs += elapsed;
            }
            self.last_playhead_update = std::time::Instant::now();

            // Punch-in / punch-out scheduling.
            if let Some(track_id) = self.pending_record_track {
                if !punch_waiting(
                    self.punch_enabled,
                    self.punch_in_secs,
                    self.punch_out_secs,
                    self.playhead_secs,
                ) {
                    if self.punch_enabled
                        && self.punch_out_secs > self.punch_in_secs
                        && self.playhead_secs >= self.punch_out_secs
                    {
                        self.pending_record_track = None;
                        self.error_message =
                            Some("The punch range has already passed.".to_string());
                    } else {
                        self.pending_record_track = None;
                        self.begin_recording(track_id);
                    }
                }
            }
            if self.recording
                && self.punch_enabled
                && self.punch_out_secs > self.punch_in_secs
                && self.playhead_secs >= self.punch_out_secs
            {
                // End the take but keep the transport rolling.
                self.stop_recording_and_create_clip();
            }
        }

        // Periodic disk-space check while recording.
        if self.recording && self.last_disk_check.elapsed().as_secs_f64() >= 1.0 {
            self.last_disk_check = std::time::Instant::now();
            self.disk_warning = self.low_disk_warning();
        }

        // MIDI audition and capture. Events are drained every frame so the
        // live sine synth can respond even when the transport is stopped.
        self.sync_live_midi_gains();
        self.poll_piano_roll_preview();
        if let Some(activity) = self
            .midi_monitor
            .as_ref()
            .map(|m| m.activity_for_channel(self.midi_input_channel))
        {
            self.midi_activity = activity;
        }
        let audition = self.midi_audition_enabled();
        let piano_roll_open =
            self.piano_roll.is_some() || self.piano_roll_preview.is_some();
        if audition != self.midi_audition_on {
            self.midi_audition_on = audition;
            if !audition {
                self.engine.all_notes_off();
            }
        }
        if (audition || piano_roll_open) && !self.engine.is_running() {
            let _ = self.engine.start();
        } else if !audition
            && !piano_roll_open
            && !self.playing
            && !self.recording
            && !self.monitoring_on
            && self.engine.is_running()
        {
            let _ = self.engine.stop();
        }
        if let Some(monitor) = &self.midi_monitor {
            let events = filter_midi_events(
                monitor.drain_events(),
                self.midi_input_channel,
            );
            if self.recording_midi {
                self.midi_captured.extend(events.iter().copied());
            }
            for event in &events {
                self.send_midi_thru(event);
                if event.on {
                    if audition {
                        self.engine.note_on(event.note, event.velocity.max(1));
                    }
                } else {
                    self.engine.note_off(event.note);
                }
            }
        }

        // Autosave every two minutes while there are unsaved changes.
        if self.last_autosave.elapsed().as_secs_f64() >= 120.0 {
            self.last_autosave = std::time::Instant::now();
            if self.project.modified && !self.recording {
                self.autosave_project();
            }
        }

        // File menu
        let recent_projects = self.recent_projects.clone();
        let mut file_action: Option<FileAction> = None;
        let midi_active = self.midi_activity.active_notes > 0;
        let midi_has_note = self.midi_activity.last_note.is_some();
        let midi_activity_text = match self.midi_activity.last_note {
            Some(note) if midi_active => format!(
                "{} vel {}",
                daw_engine::midi_note_name(note),
                self.midi_activity.last_velocity
            ),
            Some(note) => daw_engine::midi_note_name(note),
            None => "--".to_string(),
        };
        let midi_activity_color = if midi_active {
            egui::Color32::GREEN
        } else if midi_has_note {
            egui::Color32::from_gray(180)
        } else {
            egui::Color32::GRAY
        };
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        file_action = Some(FileAction::New);
                        ui.close_menu();
                    }
                    ui.menu_button("New from Template", |ui| {
                        for template in [
                            Template::Empty,
                            Template::Band,
                            Template::Electronic,
                            Template::Scoring,
                        ] {
                            if ui.button(template.label()).clicked() {
                                file_action = Some(FileAction::NewTemplate(template));
                                ui.close_menu();
                            }
                        }
                    });
                    if ui.button("Open…").clicked() {
                        file_action = Some(FileAction::Open);
                        ui.close_menu();
                    }
                    ui.menu_button("Open Recent", |ui| {
                        if recent_projects.is_empty() {
                            ui.weak("No recent projects");
                        }
                        for path in &recent_projects {
                            let name = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("project");
                            if ui.button(name).clicked() {
                                file_action = Some(FileAction::OpenPath(path.clone()));
                                ui.close_menu();
                            }
                        }
                        if !recent_projects.is_empty() {
                            ui.separator();
                            if ui.button("Clear list").clicked() {
                                file_action = Some(FileAction::ClearRecent);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Save").clicked() {
                        file_action = Some(FileAction::Save);
                        ui.close_menu();
                    }
                    if ui.button("Save As…").clicked() {
                        file_action = Some(FileAction::SaveAs);
                        ui.close_menu();
                    }
                });
                ui.menu_button("MIDI", |ui| {
                    ui.label(format!(
                        "Source: {}",
                        self.selected_midi_input.as_deref().unwrap_or("None")
                    ));
                    ui.menu_button("Source", |ui| {
                        let mut selected = self.selected_midi_input.clone();
                        let mut changed = false;
                        if ui
                            .selectable_value(&mut selected, None, "None")
                            .changed()
                        {
                            changed = true;
                        }
                        if self.midi_inputs.is_empty() {
                            ui.weak("No MIDI inputs detected");
                        } else {
                            for device in &self.midi_inputs {
                                if ui
                                    .selectable_value(
                                        &mut selected,
                                        Some(device.name.clone()),
                                        &device.name,
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("Refresh").clicked() {
                            self.refresh_midi_inputs();
                            ui.close_menu();
                        }
                        if changed {
                            self.selected_midi_input = selected;
                            self.restart_midi_monitor();
                            ui.close_menu();
                        }
                    });
                    ui.label(format!(
                        "Channel: {}",
                        self.midi_input_channel
                            .map(|channel| (channel + 1).to_string())
                            .unwrap_or_else(|| "All".to_string())
                    ));
                    ui.menu_button("Channel", |ui| {
                        let mut selected = self.midi_input_channel;
                        let mut changed = false;
                        if ui
                            .selectable_value(&mut selected, None, "All")
                            .changed()
                        {
                            changed = true;
                        }
                        for display in 1..=16u8 {
                            if ui
                                .selectable_value(
                                    &mut selected,
                                    Some(display - 1),
                                    format!("{}", display),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                        if changed {
                            self.midi_input_channel = selected;
                            ui.close_menu();
                        }
                    });
                    ui.label(format!(
                        "Output: {}",
                        self.selected_midi_output.as_deref().unwrap_or("None")
                    ));
                    ui.menu_button("Output", |ui| {
                        let mut selected = self.selected_midi_output.clone();
                        let mut changed = false;
                        if ui
                            .selectable_value(&mut selected, None, "None")
                            .changed()
                        {
                            changed = true;
                        }
                        if self.midi_outputs.is_empty() {
                            ui.weak("No MIDI outputs detected");
                        } else {
                            for device in &self.midi_outputs {
                                if ui
                                    .selectable_value(
                                        &mut selected,
                                        Some(device.name.clone()),
                                        &device.name,
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("Refresh").clicked() {
                            self.refresh_midi_outputs();
                            ui.close_menu();
                        }
                        if changed {
                            self.selected_midi_output = selected;
                            if self.selected_midi_output.is_none() {
                                self.midi_thru_enabled = false;
                            }
                            self.restart_midi_thru();
                            ui.close_menu();
                        }
                    });
                    if ui
                        .add_enabled(
                            self.selected_midi_output.is_some(),
                            egui::Checkbox::new(
                                &mut self.midi_thru_enabled,
                                "MIDI Thru",
                            ),
                        )
                        .on_hover_text(
                            "Echo input notes to the selected MIDI output",
                        )
                        .changed()
                    {
                        self.restart_midi_thru();
                    }
                    ui.separator();
                    if ui.button("Panic").clicked() {
                        self.panic_midi();
                        ui.close_menu();
                    }
                    ui.checkbox(&mut self.input_quantize, "Input Quantize")
                        .on_hover_text(
                            "Quantize recorded MIDI notes to the Arrange snap grid",
                        );
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Activity:");
                        ui.colored_label(midi_activity_color, &midi_activity_text);
                    });
                });
            });
        });
        if let Some(action) = file_action {
            self.handle_file_action(action);
        }

        // Error banner (shown when an operation, e.g. recording, fails)
        if let Some(message) = self.error_message.clone() {
            egui::TopBottomPanel::top("error_banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 90, 90),
                        format!("Error: {}", message),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Dismiss").clicked() {
                            self.error_message = None;
                        }
                    });
                });
            });
        }

        if let Some(message) = self.disk_warning.clone() {
            egui::TopBottomPanel::top("disk_warning_banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 200, 80),
                        format!("Warning: {}", message),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Dismiss").clicked() {
                            self.disk_warning = None;
                        }
                    });
                });
            });
        }

        if let Some(path) = self.recovery_offer.clone() {
            egui::TopBottomPanel::top("recovery_banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 190, 255),
                        format!(
                            "A newer autosave was found: {}",
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("autosave.zoft")
                        ),
                    );
                    if ui.button("Load recovery").clicked() {
                        self.load_recovery(&path);
                    }
                    if ui.button("Dismiss").clicked() {
                        self.recovery_offer = None;
                    }
                });
            });
        }

        // Top transport bar
        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸" } else { "▶" }).clicked() {
                    self.toggle_playback();
                }
                if ui
                    .button("|<")
                    .on_hover_text("Jump to the start (Home)")
                    .clicked()
                {
                    self.pause_transport();
                    self.playhead_secs = 0.0;
                }
                if ui
                    .button(">|")
                    .on_hover_text("Jump to the end (End)")
                    .clicked()
                {
                    self.pause_transport();
                    self.playhead_secs = self.timeline_duration();
                }
                ui.separator();

                // Record button (red while a take is recording or armed)
                let record_active = self.recording || self.pending_record_track.is_some();
                let record_text = if self.recording { "⏹" } else { "●" };
                let record_color = if record_active {
                    egui::Color32::RED
                } else {
                    egui::Color32::WHITE
                };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(record_text).color(record_color))
                            .min_size([32.0, 24.0].into()),
                    )
                    .clicked()
                {
                    self.toggle_recording();
                }

                if self.recording {
                    if let Some((peak, rms)) = self.engine.input_level() {
                        ui.separator();
                        draw_input_meter(ui, peak, rms);
                    }
                }

                ui.separator();

                // Transport loop: In/Out set the range at the playhead; Loop
                // repeats it during playback.
                if ui
                    .selectable_label(self.loop_enabled, "Loop")
                    .on_hover_text("Repeat playback between the In and Out points")
                    .clicked()
                {
                    self.loop_enabled = !self.loop_enabled;
                    self.loop_changed();
                }
                if ui
                    .small_button("In")
                    .on_hover_text("Set loop start at the playhead")
                    .clicked()
                {
                    self.loop_start_secs = self.playhead_secs.max(0.0);
                    if self.loop_end_secs <= self.loop_start_secs {
                        self.loop_end_secs = self.loop_start_secs + 1.0;
                    }
                    self.loop_enabled = true;
                    self.loop_changed();
                }
                if ui
                    .small_button("Out")
                    .on_hover_text("Set loop end at the playhead")
                    .clicked()
                {
                    self.loop_end_secs = self.playhead_secs.max(0.0);
                    if self.loop_end_secs <= self.loop_start_secs {
                        self.loop_start_secs = (self.loop_end_secs - 1.0).max(0.0);
                    }
                    self.loop_enabled = true;
                    self.loop_changed();
                }
                ui.label(format!(
                    "{} – {}",
                    format_clock(self.loop_start_secs),
                    format_clock(self.loop_end_secs)
                ));
                if ui
                    .selectable_label(self.punch_enabled, "Punch")
                    .on_hover_text("Record only between the punch points")
                    .clicked()
                {
                    self.punch_enabled = !self.punch_enabled;
                    if self.punch_enabled && self.punch_out_secs <= self.punch_in_secs {
                        self.punch_in_secs = 0.0;
                        self.punch_out_secs = self.timeline_duration().max(4.0);
                    }
                }
                if ui
                    .small_button("P-In")
                    .on_hover_text("Set punch-in at the playhead")
                    .clicked()
                {
                    self.punch_in_secs = self.playhead_secs.max(0.0);
                    if self.punch_out_secs <= self.punch_in_secs {
                        self.punch_out_secs = self.punch_in_secs + 4.0;
                    }
                    self.punch_enabled = true;
                }
                if ui
                    .small_button("P-Out")
                    .on_hover_text("Set punch-out at the playhead")
                    .clicked()
                {
                    self.punch_out_secs = self.playhead_secs.max(0.0);
                    if self.punch_out_secs <= self.punch_in_secs {
                        self.punch_in_secs = (self.punch_out_secs - 1.0).max(0.0);
                    }
                    self.punch_enabled = true;
                }
                if self.punch_enabled {
                    ui.label(format!(
                        "{} – {}",
                        format_clock(self.punch_in_secs),
                        format_clock(self.punch_out_secs)
                    ));
                }
                if ui
                    .selectable_label(self.metronome_enabled, "Click")
                    .on_hover_text("Metronome click during playback")
                    .clicked()
                {
                    self.metronome_enabled = !self.metronome_enabled;
                    self.sync_playback_metronome();
                }
                if ui
                    .small_button("+ Marker")
                    .on_hover_text("Add a marker at the playhead (Shift+M)")
                    .clicked()
                {
                    self.add_marker_at_playhead();
                }

                ui.separator();
                ui.label(format!("Project: {}", self.project.name));
                if let Some(status) = &self.autosave_status {
                    ui.weak(status);
                }
                ui.separator();
                ui.label(format!("Sample Rate: {} Hz", self.project.sample_rate));
                ui.separator();

                // Time signature picker. This drives the bars/beats ruler, the
                // snap grid and the musical length of a bar.
                ui.label("Time Sig:");
                let (current_num, current_den) = self.time_signature();
                let mut chosen = (current_num, current_den);
                egui::ComboBox::from_id_salt("time_signature")
                    .selected_text(format!("{}/{}", current_num, current_den))
                    .width(58.0)
                    .show_ui(ui, |ui| {
                        for (num, den) in TIME_SIGNATURES {
                            ui.selectable_value(
                                &mut chosen,
                                (num, den),
                                format!("{}/{}", num, den),
                            );
                        }
                    });
                if chosen != (current_num, current_den) {
                    self.project
                        .time_signature_map
                        .set_change(0, chosen.0, chosen.1);
                    self.push_undo(
                        EditAction::TimeSignature {
                            from: (current_num, current_den),
                            to: chosen,
                        },
                        false,
                    );
                    self.after_edit();
                    self.sync_playback_metronome();
                }
                ui.separator();

                // Tempo: drag or type a BPM, or tap it in.
                ui.label("Tempo:");
                let mut bpm = self.project.tempo_map.default_bpm();
                let bpm_response = ui.add(
                    egui::DragValue::new(&mut bpm)
                        .speed(0.1)
                        .range(20.0..=300.0)
                        .max_decimals(1)
                        .suffix(" BPM"),
                );
                if bpm_response.changed() {
                    if self.bpm_edit_from.is_none() {
                        self.bpm_edit_from = Some(self.project.tempo_map.default_bpm());
                    }
                    self.project.tempo_map.set_default_bpm(bpm);
                    self.sync_playback_metronome();
                }
                let finished_edit = bpm_response.drag_stopped()
                    || (bpm_response.changed() && !bpm_response.dragged());
                if finished_edit {
                    if let Some(from) = self.bpm_edit_from.take() {
                        let to = self.project.tempo_map.default_bpm();
                        if (to - from).abs() > 1e-4 {
                            self.push_undo(EditAction::Tempo { from, to }, false);
                            self.after_edit();
                        }
                    }
                }
                if ui
                    .small_button("Tap")
                    .on_hover_text("Tap four times to set the tempo")
                    .clicked()
                {
                    self.tap_tempo();
                }
                ui.separator();

                // Input device selector
                if !self.input_devices.is_empty() {
                    ui.separator();
                    ui.label("Input:");
                    let mut selected_changed = false;
                    let mut new_selection = self.selected_input_device.clone();
                    egui::ComboBox::from_label("")
                        .selected_text(self.selected_input_device.as_deref().unwrap_or("Default"))
                        .show_ui(ui, |ui| {
                            for device in &self.input_devices {
                                let is_default = if device.is_default_input { " (default)" } else { "" };
                                if ui
                                    .selectable_value(
                                        &mut new_selection,
                                        Some(device.id.clone()),
                                        format!("{}{}", device.name, is_default),
                                    )
                                    .changed()
                                {
                                    selected_changed = true;
                                }
                            }
                        });
                    if selected_changed {
                        let _ = self.engine.set_selected_input_device(new_selection.clone());
                        self.selected_input_device = new_selection;
                        // Restart the input stream so monitoring uses the new
                        // device immediately.
                        self.engine.set_monitoring(false);
                        self.sync_monitoring();
                    }
                }
                ui.separator();
                ui.label(format!("Tracks: {}", self.project.track_order.len()));
            });
        });

        // Arrange toolbar (above the side panel and timeline so rows line up)
        egui::TopBottomPanel::top("arrange_toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Arrange");
                ui.separator();
                if ui.small_button("Fit").clicked() {
                    self.needs_fit = true;
                }
                if ui.small_button("−").clicked() {
                    self.zoom_center(1.0 / 1.5);
                }
                if ui.small_button("+").clicked() {
                    self.zoom_center(1.5);
                }
                ui.label(format!("{:.1} px/s", self.pixels_per_sec));
                ui.separator();

                // Undo / redo
                let can_undo = self.history.can_undo();
                let can_redo = self.history.can_redo();
                if ui
                    .add_enabled(can_undo, egui::Button::new("↶"))
                    .on_hover_text("Undo (Ctrl+Z)")
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("↷"))
                    .on_hover_text("Redo (Ctrl+Shift+Z)")
                    .clicked()
                {
                    self.redo();
                }
                if ui
                    .selectable_label(self.history_open, "History")
                    .on_hover_text("Show every edit in this session and jump to any point")
                    .clicked()
                {
                    self.history_open = !self.history_open;
                }
                if ui
                    .add_enabled(
                        !self.selected_clips.is_empty(),
                        egui::Button::new("Audio…"),
                    )
                    .on_hover_text("Clip gain, fades, colour and effects")
                    .clicked()
                {
                    let ids = self.selection_ids();
                    if !ids.is_empty() {
                        self.audio_window = Some(ClipAudioWindow::new(ids));
                    }
                }
                if ui
                    .add_enabled(
                        self.selected_midi_clip.is_some(),
                        egui::Button::new("Piano Roll…"),
                    )
                    .on_hover_text("Inspect the selected MIDI clip")
                    .clicked()
                {
                    if let Some(clip_id) = self.selected_midi_clip {
                        self.piano_roll = Some(PianoRollWindow::new(clip_id, self.snap_mode));
                    }
                }
                if ui
                    .selectable_label(self.fx_rack_open, "FX Rack")
                    .on_hover_text("Show every track's insert chain (X)")
                    .clicked()
                {
                    if self.fx_rack_open {
                        self.commit_pending_insert_edit();
                    }
                    self.fx_rack_open = !self.fx_rack_open;
                }
                ui.separator();
                ui.label("Snap:");
                egui::ComboBox::from_id_salt("snap_mode")
                    .selected_text(self.snap_mode.label())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for mode in [
                            SnapMode::Off,
                            SnapMode::Bar,
                            SnapMode::Beat,
                            SnapMode::Eighth,
                            SnapMode::Sixteenth,
                        ] {
                            ui.selectable_value(&mut self.snap_mode, mode, mode.label());
                        }
                    });
                ui.checkbox(&mut self.snap_relative, "Relative").on_hover_text(
                    "Move clips in grid steps relative to their original position, preserving off-grid offsets",
                );
                ui.separator();

                // Ruler display: musical bars/beats (default) or clock time.
                ui.label("Ruler:");
                egui::ComboBox::from_id_salt("ruler_mode")
                    .selected_text(self.ruler_mode.label())
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for mode in [RulerMode::BarsBeats, RulerMode::Time] {
                            ui.selectable_value(&mut self.ruler_mode, mode, mode.label());
                        }
                    });
                ui.separator();
                ui.label(format!(
                    "{} / {}",
                    format_clock(self.playhead_secs),
                    format_clock(self.timeline_duration())
                ));
            });
        });

        // Left track list
        egui::SidePanel::left("track_list")
            .default_width(420.0)
            .min_width(320.0)
            // Match the central panel's vertical inner margin so the first row
            // starts at the same y as the arrange ruler's first lane.
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::same(8.0)),
            )
            .show(ctx, |ui| {
            let mut add_audio_track = false;
            let mut add_midi_track = false;
            let mut remove_track = None;
            let mut open_fx_rack = false;
            let mut ghost_changes: Vec<(TrackId, bool)> = Vec::new();
            ui.spacing_mut().item_spacing.y = 0.0;
            let (header_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), RULER_HEIGHT),
                egui::Sense::hover(),
            );
            let mut header_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("track_list_header")
                    .max_rect(header_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            header_ui.add_space(4.0);
            header_ui.heading("Tracks");
            header_ui.add_space(4.0);
            if header_ui
                .small_button("+Audio")
                .on_hover_text("Add audio track")
                .clicked()
            {
                add_audio_track = true;
            }
            if header_ui
                .small_button("+MIDI")
                .on_hover_text("Add MIDI track")
                .clicked()
            {
                add_midi_track = true;
            }
            let mut track_changes: Vec<(TrackId, TrackShape, TrackShape, bool)> = Vec::new();
            let track_rows_top = ui.cursor().top();
            let mut track_drag_started = None;
            let mut track_drag_current = None;
            let mut track_drag_stopped: Option<(TrackId, f32)> = None;
            for track_id in &self.project.track_order {
                if let Some(track) = self.project.tracks.get(track_id) {
                    let before = TrackShape::of(track);
                    let mut after = before;
                    // Reserve the full lane height before laying out the row.
                    // `allocate_ui_with_layout` only advances by the content's
                    // height, which would make the left rows shorter than the
                    // arrange lanes and break vertical alignment.
                    let (row_rect, row_response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), TRACK_HEIGHT),
                        egui::Sense::hover(),
                    );
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("track_row", *track_id))
                            .max_rect(row_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    row_ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                    let row = egui::InnerResponse::new(
                        (|ui: &mut egui::Ui| {
                            let content_height = 20.0 + 2.0 + 20.0 + 2.0 + 26.0;
                            ui.add_space(
                                ((row_rect.height() - content_height) * 0.5)
                                    .max(0.0),
                            );

                            // Top line: track identity and mute/solo/record.
                            ui.horizontal(|ui| {
                                let handle = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new("::")
                                                .color(egui::Color32::from_gray(130)),
                                        )
                                        .sense(egui::Sense::drag()),
                                    )
                                    .on_hover_text("Drag to reorder track");
                                if handle.hovered() {
                                    ui.output_mut(|output| {
                                        output.cursor_icon =
                                            egui::CursorIcon::Grab;
                                    });
                                }
                                if handle.drag_started() {
                                    track_drag_started = Some(*track_id);
                                }
                                if handle.dragged() {
                                    ui.output_mut(|output| {
                                        output.cursor_icon =
                                            egui::CursorIcon::Grabbing;
                                    });
                                    track_drag_current = Some((
                                        *track_id,
                                        handle
                                            .interact_pointer_pos()
                                            .map(|pos| pos.y),
                                    ));
                                }
                                if handle.drag_stopped() {
                                    let y = handle
                                        .interact_pointer_pos()
                                        .or_else(|| {
                                            ui.input(|i| i.pointer.latest_pos())
                                        })
                                        .map(|pos| pos.y);
                                    if let Some(y) = y {
                                        track_drag_stopped =
                                            Some((*track_id, y));
                                    }
                                }
                                let color = egui::Color32::from_rgb(
                                    track.color.0,
                                    track.color.1,
                                    track.color.2,
                                );
                                ui.colored_label(color, "■");
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&track.name).strong(),
                                    )
                                    .truncate(),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(
                                        egui::Align::Center,
                                    ),
                                    |ui| {
                                        let arm_text = if after.record_arm {
                                            "●"
                                        } else {
                                            "○"
                                        };
                                        let arm_color = if after.record_arm {
                                            egui::Color32::RED
                                        } else {
                                            egui::Color32::GRAY
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(arm_text)
                                                        .color(arm_color),
                                                )
                                                .min_size([20.0, 20.0].into()),
                                            )
                                            .on_hover_text("Record arm")
                                            .clicked()
                                        {
                                            after.record_arm = !after.record_arm;
                                        }

                                        let solo_color = if after.solo {
                                            egui::Color32::GREEN
                                        } else {
                                            egui::Color32::GRAY
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("S")
                                                        .color(solo_color),
                                                )
                                                .min_size([20.0, 20.0].into()),
                                            )
                                            .on_hover_text("Solo")
                                            .clicked()
                                        {
                                            after.solo = !after.solo;
                                        }

                                        let mute_color = if after.mute {
                                            egui::Color32::YELLOW
                                        } else {
                                            egui::Color32::GRAY
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("M")
                                                        .color(mute_color),
                                                )
                                                .min_size([20.0, 20.0].into()),
                                            )
                                            .on_hover_text("Mute")
                                            .clicked()
                                        {
                                            after.mute = !after.mute;
                                        }
                                    },
                                );
                            });

                            // Bottom line: routing, effects, ghost visibility,
                            // volume and remove.
                            ui.horizontal(|ui| {
                                let monitor_color = match after.monitor_mode {
                                    MonitorMode::Off => egui::Color32::GRAY,
                                    MonitorMode::Auto => egui::Color32::YELLOW,
                                    MonitorMode::On => egui::Color32::GREEN,
                                };
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Mon")
                                                .color(monitor_color),
                                        )
                                        .min_size([30.0, 20.0].into()),
                                    )
                                    .on_hover_text(
                                        "Input monitoring: Off → Auto (armed or recording) → On",
                                    )
                                    .clicked()
                                {
                                    after.monitor_mode = match after.monitor_mode {
                                        MonitorMode::Off => MonitorMode::Auto,
                                        MonitorMode::Auto => MonitorMode::On,
                                        MonitorMode::On => MonitorMode::Off,
                                    };
                                }

                                if matches!(
                                    track.track_type,
                                    TrackType::Midi | TrackType::Instrument
                                ) {
                                    let ghost_color = if track.show_as_ghost {
                                        egui::Color32::from_rgb(120, 170, 240)
                                    } else {
                                        egui::Color32::GRAY
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("G")
                                                    .color(ghost_color),
                                            )
                                            .min_size([20.0, 20.0].into()),
                                        )
                                        .on_hover_text(
                                            "Include this track's notes as piano-roll ghost notes",
                                        )
                                        .clicked()
                                    {
                                        ghost_changes.push((
                                            *track_id,
                                            !track.show_as_ghost,
                                        ));
                                    }
                                } else {
                                    ui.add_space(20.0);
                                }

                                ui.label("Vol");
                                add_sized_wheel_slider_step(
                                    ui,
                                    egui::vec2(100.0, 18.0),
                                    &mut after.volume,
                                    0.0..=1.5,
                                    Some(0.05),
                                    false,
                                    |slider| slider.show_value(false),
                                )
                                .on_hover_text(format!(
                                    "Track volume {:.0}%",
                                    after.volume * 100.0
                                ));
                                ui.label(format!(
                                    "{:.0}%",
                                    after.volume * 100.0
                                ));
                            });

                            // Third line: pan fader, aligned under the volume
                            // slider. The leading space matches Mon + G.
                            ui.horizontal(|ui| {
                                ui.add_space(58.0);
                                ui.label("Pan");
                                let mut pan = after.pan.to_linear();
                                if compact_knob(
                                    ui,
                                    &mut pan,
                                    -1.0..=1.0,
                                    0.0,
                                    false,
                                )
                                .changed()
                                {
                                    after.pan = PanLaw::from_linear(pan);
                                }
                                let pan_text = if pan.abs() < 0.005 {
                                    "C".to_string()
                                } else if pan < 0.0 {
                                    format!("L{:.0}", -pan * 100.0)
                                } else {
                                    format!("R{:.0}", pan * 100.0)
                                };
                                ui.label(pan_text);
                                if ui
                                    .small_button("×")
                                    .on_hover_text("Remove track")
                                    .clicked()
                                {
                                    remove_track = Some(*track_id);
                                }
                            });
                        })(&mut row_ui),
                        row_response,
                    );
                    row.response.context_menu(|ui| {
                        if ui.button("Show FX Rack").clicked() {
                            open_fx_rack = true;
                            ui.close_menu();
                        }
                        if matches!(
                            track.track_type,
                            TrackType::Midi | TrackType::Instrument
                        ) {
                            ui.separator();
                            let mut show_as_ghost = track.show_as_ghost;
                            if ui
                                .checkbox(
                                    &mut show_as_ghost,
                                    "Show as ghost notes",
                                )
                                .changed()
                            {
                                ghost_changes.push((*track_id, show_as_ghost));
                            }
                        }
                        ui.separator();
                        if ui.button("Remove Track").clicked() {
                            remove_track = Some(*track_id);
                            ui.close_menu();
                        }
                    });
                    ui.painter().hline(
                        row.response.rect.x_range(),
                        row.response.rect.bottom(),
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(50)),
                    );
                    if after != before {
                        let slider_only = after.mute == before.mute
                            && after.solo == before.solo
                            && after.record_arm == before.record_arm
                            && after.monitor_mode == before.monitor_mode
                            && ((after.volume - before.volume).abs()
                                > f32::EPSILON
                                || after.pan != before.pan);
                        track_changes.push((*track_id, before, after, slider_only));
                    }
                }
            }
            let track_count = self.project.track_order.len();
            let track_target_at = |y: f32| -> usize {
                (((y - track_rows_top) / TRACK_HEIGHT).floor() as i64)
                    .clamp(0, track_count.saturating_sub(1) as i64)
                    as usize
            };
            let track_drag_was_stopped = track_drag_stopped.is_some();
            if let Some((track_id, y)) = track_drag_stopped {
                let from_index = self
                    .project
                    .track_order
                    .iter()
                    .position(|id| *id == track_id);
                let to_index = track_target_at(y);
                if let Some(from_index) = from_index {
                    if from_index != to_index {
                        let action = EditAction::MoveTrack {
                            track_id,
                            from_index,
                            to_index,
                        };
                        self.apply_action(&action, true);
                        self.push_undo(action, false);
                        self.after_edit();
                    }
                }
                self.track_drag = None;
            } else if let Some(track_id) = track_drag_started {
                self.track_drag = Some(track_id);
            }
            if !track_drag_was_stopped {
                if let Some((track_id, pointer_y)) = track_drag_current {
                    self.track_drag = Some(track_id);
                    if let Some(y) = pointer_y {
                        let target = track_target_at(y);
                        let line_y =
                            track_rows_top + target as f32 * TRACK_HEIGHT;
                        ui.painter().hline(
                            ui.max_rect().x_range(),
                            line_y,
                            egui::Stroke::new(
                                2.0_f32,
                                egui::Color32::from_rgb(120, 170, 240),
                            ),
                        );
                    }
                }
            }
            for (track_id, from, to, merge) in track_changes {
                if let Some(track) = self.project.tracks.get_mut(&track_id) {
                    to.apply(track);
                }
                self.push_undo(EditAction::TrackState { track_id, from, to }, merge);
                self.after_edit();
                if merge {
                    // Update fader/pan gains on the live mixer without
                    // reloading clips from disk.
                    let tracks = self.collect_playable_tracks();
                    self.engine.set_playback_tracks(tracks);
                } else {
                    self.refresh_playback_if_playing();
                }
            }
            for (track_id, show_as_ghost) in ghost_changes {
                if let Some(track) = self.project.tracks.get_mut(&track_id) {
                    if track.show_as_ghost != show_as_ghost {
                        track.show_as_ghost = show_as_ghost;
                        self.project.modified = true;
                    }
                }
            }
            if add_audio_track {
                self.add_track(false);
            }
            if add_midi_track {
                self.add_track(true);
            }
            if let Some(track_id) = remove_track {
                self.remove_track(track_id);
            }
            if open_fx_rack {
                self.fx_rack_open = true;
            }
            self.sync_monitoring();

            // ---- Trash browser ----
            // Deleting a clip only detaches it from its track; the audio and
            // clip data stay in the project and can be restored here.
            ui.separator();
            let attached: HashSet<ClipId> = self
                .project
                .tracks
                .values()
                .flat_map(|track| track.clips.iter().copied())
                .collect();
            let trash: Vec<(ClipId, String, f64)> = self
                .project
                .audio_clips
                .iter()
                .filter(|(id, _)| !attached.contains(id))
                .map(|(id, clip)| {
                    let name = clip
                        .source
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("clip")
                        .to_string();
                    let secs =
                        clip.length as f64 / clip.source.sample_rate.max(1) as f64;
                    (id, name, secs)
                })
                .collect();
            egui::CollapsingHeader::new(format!("Trash ({})", trash.len()))
                .default_open(false)
                .show(ui, |ui| {
                    if trash.is_empty() {
                        ui.weak("Deleted clips appear here and can be restored.");
                        return;
                    }
                    for (clip_id, name, secs) in &trash {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} ({:.2}s)", name, secs));
                            if ui.small_button("Restore").clicked() {
                                self.restore_clip(*clip_id);
                            }
                        });
                    }
                });
        });

        // Central timeline/arrange area
        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            if rect.width() <= 1.0 || rect.height() <= 1.0 {
                return;
            }
            self.last_timeline_width = rect.width();

            if self.needs_fit {
                self.fit_view(rect.width());
            }

            // ---- Zoom / scroll input ----
            let hovered = ui.rect_contains_pointer(rect);
            let (zoom_delta, scroll, pointer_pos, middle_delta) = ui.input(|i| {
                (
                    i.zoom_delta(),
                    i.smooth_scroll_delta,
                    i.pointer.hover_pos(),
                    if i.pointer.button_down(egui::PointerButton::Middle) {
                        i.pointer.delta()
                    } else {
                        egui::Vec2::ZERO
                    },
                )
            });
            if hovered {
                if (zoom_delta - 1.0).abs() > 0.001 {
                    let px = pointer_pos.map(|p| p.x).unwrap_or(rect.center().x);
                    self.zoom_at(rect, px, zoom_delta);
                } else if scroll.x != 0.0 || scroll.y != 0.0 {
                    let pps = self.pixels_per_sec.max(MIN_PPS);
                    self.view_start_secs -= (scroll.x + scroll.y) as f64 / pps as f64;
                }
            }
            if hovered && middle_delta != egui::Vec2::ZERO {
                self.view_start_secs -= middle_delta.x as f64 / self.pixels_per_sec.max(MIN_PPS) as f64;
            }
            self.clamp_view(rect.width());

            // Follow the playhead while playing: page the view forward when the
            // playhead reaches the right edge (it may run past the last clip).
            if self.playing {
                let ph_x = rect.left()
                    + ((self.playhead_secs - self.view_start_secs) * self.pixels_per_sec as f64)
                        as f32;
                if ph_x > rect.right() - 80.0 {
                    let view_len = rect.width() as f64 / self.pixels_per_sec.max(MIN_PPS) as f64;
                    self.view_start_secs = self.playhead_secs - view_len * 0.25;
                    self.clamp_view(rect.width());
                }
            }

            let view = TimelineView {
                start_secs: self.view_start_secs,
                pps: self.pixels_per_sec,
            };
            let painter = ui.painter();
            let project_sr = self.project.sample_rate.max(1) as f64;
            let markers = self.project.marker_regions.clone();
            let mut marker_flags: Vec<(MarkerRegion, egui::Rect, f64)> = Vec::new();
            let track_height = TRACK_HEIGHT;
            let ruler_height = RULER_HEIGHT;
            // Upper part of the ruler is the loop lane (drag to set the loop,
            // like Logic's cycle area); the lower part seeks the playhead.
            let loop_lane_bottom = rect.top() + ruler_height * 0.4;

            // ---- Ruler ----
            let ruler_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + ruler_height),
            );
            painter.rect_filled(ruler_rect, 0.0, egui::Color32::from_gray(25));
            painter.line_segment(
                [
                    egui::pos2(rect.left(), loop_lane_bottom),
                    egui::pos2(rect.right(), loop_lane_bottom),
                ],
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(48)),
            );

            // ---- Loop region ----
            if self.loop_enabled && self.loop_end_secs > self.loop_start_secs {
                let loop_start = view.x(rect, self.loop_start_secs);
                let loop_end = view.x(rect, self.loop_end_secs);
                let band = egui::Rect::from_min_max(
                    egui::pos2(loop_start, rect.top()),
                    egui::pos2(loop_end, loop_lane_bottom),
                )
                .intersect(ruler_rect);
                if band.width() > 0.0 {
                    painter.rect_filled(band, 0.0, egui::Color32::from_rgb(42, 68, 104));
                }
                for x in [loop_start, loop_end] {
                    if x >= rect.left() && x <= rect.right() {
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(80, 130, 190)),
                        );
                    }
                }
            }

            // ---- Punch region: a red bar at the bottom of the ruler ----
            if self.punch_enabled && self.punch_out_secs > self.punch_in_secs {
                let punch_in = view.x(rect, self.punch_in_secs);
                let punch_out = view.x(rect, self.punch_out_secs);
                let band = egui::Rect::from_min_max(
                    egui::pos2(punch_in, rect.top() + ruler_height - 3.0),
                    egui::pos2(punch_out, rect.top() + ruler_height),
                )
                .intersect(ruler_rect);
                if band.width() > 0.0 {
                    painter.rect_filled(band, 0.0, egui::Color32::from_rgb(170, 60, 60));
                }
                for x in [punch_in, punch_out] {
                    if x >= rect.left() && x <= rect.right() {
                        painter.line_segment(
                            [
                                egui::pos2(x, rect.top() + ruler_height - 6.0),
                                egui::pos2(x, rect.top() + ruler_height),
                            ],
                            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(200, 80, 80)),
                        );
                    }
                }
            }

            let view_end = self.view_start_secs + view.visible_secs(rect);
            let ruler_ticks = self.build_ruler_ticks(self.view_start_secs, view_end);

            // Minor ticks
            for tick in ruler_ticks.iter().filter(|tick| !tick.major) {
                let x = view.x(rect, tick.secs);
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top() + ruler_height * 0.7),
                        egui::pos2(x, rect.top() + ruler_height),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(60)),
                );
            }

            // Major ticks with labels
            for tick in ruler_ticks.iter().filter(|tick| tick.major) {
                let x = view.x(rect, tick.secs);
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top()),
                        egui::pos2(x, rect.top() + ruler_height),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)),
                );
                if let Some(label) = &tick.label {
                    painter.text(
                        egui::pos2(x + 3.0, rect.top() + 4.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(180),
                    );
                }
            }

            // ---- Markers ----
            for marker in &markers {
                let sample = self
                    .project
                    .tempo_map
                    .tick_to_sample(marker.start_tick, project_sr);
                let secs = sample as f64 / project_sr;
                let x = view.x(rect, secs);
                if x < rect.left() - 8.0 || x > rect.right() + 8.0 {
                    continue;
                }
                let color = egui::Color32::from_rgb(
                    marker.color.0,
                    marker.color.1,
                    marker.color.2,
                );
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top()),
                        egui::pos2(x, rect.bottom()),
                    ],
                    egui::Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
                );
                let flag = egui::Rect::from_min_max(
                    egui::pos2(x, rect.top() + 1.0),
                    egui::pos2((x + 78.0).min(rect.right()), rect.top() + 16.0),
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x, rect.top() + 1.0),
                        egui::pos2(x + 9.0, rect.top() + 16.0),
                    ),
                    2.0,
                    color,
                );
                painter.text(
                    egui::pos2(x + 11.0, rect.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    &marker.name,
                    egui::FontId::proportional(9.0),
                    color,
                );
                marker_flags.push((marker.clone(), flag, secs));
            }

            // ---- Ruler interaction ----
            // Upper part = loop lane: dragging sets a new loop range (Logic's
            // cycle area). Lower part = seek lane: click/drag places the
            // playhead.
            let ruler_response = ui.interact(
                ruler_rect,
                ui.id().with("ruler_seek"),
                egui::Sense::click_and_drag(),
            );

            // Marker flags sit above the ruler and take click priority.
            let mut marker_handled = false;
            let mut marker_seek = None;
            let mut marker_delete = None;
            for (marker, flag, secs) in &marker_flags {
                let response = ui.interact(
                    *flag,
                    ui.id().with(("marker_flag", marker.id.0)),
                    egui::Sense::click(),
                );
                if response.clicked() {
                    marker_seek = Some(*secs);
                    marker_handled = true;
                }
                if response.secondary_clicked() {
                    marker_delete = Some(marker.clone());
                    marker_handled = true;
                }
                response.on_hover_text(format!(
                    "{} — click to jump, right-click to delete",
                    marker.name
                ));
            }
            if let Some(secs) = marker_seek {
                if self.playing || self.recording {
                    self.pause_transport();
                }
                self.playhead_secs = secs.max(0.0);
            }
            if let Some(marker) = marker_delete {
                self.remove_marker(marker);
            }

            let playhead_x_before = view.x(rect, self.playhead_secs);
            let pointer_on_playhead = pointer_pos
                .map(|p| (p.x - playhead_x_before).abs() <= 8.0)
                .unwrap_or(false);
            let max_t = self.timeline_duration().max(view.t(rect, rect.right()));
            let pointer_in_loop_lane = pointer_pos
                .map(|p| p.y <= loop_lane_bottom)
                .unwrap_or(false);
            let pressed = ui.input(|i| i.pointer.primary_pressed());

            if ruler_response.drag_started_by(egui::PointerButton::Primary) {
                let started_in_loop_lane = ruler_response
                    .interact_pointer_pos()
                    .map(|p| p.y <= loop_lane_bottom)
                    .unwrap_or(false);
                if started_in_loop_lane {
                    if let Some(pos) = ruler_response.interact_pointer_pos() {
                        let t = view.t(rect, pos.x).clamp(0.0, max_t);
                        let anchor = self.snap_secs(t);
                        self.loop_drag_anchor_secs = Some(anchor);
                        self.loop_enabled = true;
                        self.loop_start_secs = anchor;
                        self.loop_end_secs = anchor;
                        self.sync_playback_loop();
                    }
                } else {
                    self.loop_drag_anchor_secs = None;
                }
            }

            if ruler_response.dragged_by(egui::PointerButton::Primary) {
                if let Some(anchor) = self.loop_drag_anchor_secs {
                    if let Some(pos) = ruler_response.interact_pointer_pos() {
                        let t = view.t(rect, pos.x).clamp(0.0, max_t);
                        let current = self.snap_secs(t);
                        self.loop_start_secs = anchor.min(current);
                        self.loop_end_secs = anchor.max(current);
                        self.sync_playback_loop();
                    }
                }
            }

            if ruler_response.drag_stopped_by(egui::PointerButton::Primary)
                && self.loop_drag_anchor_secs.take().is_some()
            {
                if self.loop_end_secs - self.loop_start_secs < MIN_LOOP_SECS {
                    self.loop_end_secs = self.loop_start_secs + MIN_LOOP_SECS;
                }
                self.loop_changed();
            }

            let dragging_loop = self.loop_drag_anchor_secs.is_some();
            let seek_pos = if !marker_handled
                && !dragging_loop
                && ruler_response.dragged_by(egui::PointerButton::Primary)
            {
                ruler_response.interact_pointer_pos()
            } else if !marker_handled && ruler_response.clicked() && !pointer_on_playhead {
                ruler_response.interact_pointer_pos()
            } else if !marker_handled
                && pressed
                && ruler_response.hovered()
                && !pointer_on_playhead
                && !pointer_in_loop_lane
            {
                pointer_pos
            } else {
                None
            };
            if let Some(pos) = seek_pos {
                // Seeking stops the transport (and ends an active take).
                if self.playing || self.recording {
                    self.pause_transport();
                }
                let t = view.t(rect, pos.x).clamp(0.0, max_t);
                self.playhead_secs = self.snap_secs(t);
            }
            if ruler_response.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }

            // ---- Track lanes and clips ----
            let track_layout: Vec<(egui::Color32, Vec<ClipId>, Vec<ClipId>)> = self
                .project
                .track_order
                .iter()
                .filter_map(|id| {
                    self.project.tracks.get(id).map(|track| {
                        (
                            egui::Color32::from_rgb(track.color.0, track.color.1, track.color.2),
                            track.clips.clone(),
                            track.midi_clips.clone(),
                        )
                    })
                })
                .collect();

            let track_y_start = rect.top() + ruler_height;
            let mut clip_rects: Vec<(ClipId, egui::Rect)> = Vec::new();
            let mut midi_clip_rects: Vec<(ClipId, egui::Rect)> = Vec::new();

            for (track_idx, (color, audio_clips, midi_clips)) in track_layout.iter().enumerate() {
                let track_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), track_y_start + track_idx as f32 * track_height),
                    egui::pos2(rect.right(), track_y_start + (track_idx + 1) as f32 * track_height),
                );

                let bg_color = if track_idx % 2 == 0 {
                    egui::Color32::from_gray(35)
                } else {
                    egui::Color32::from_gray(30)
                };
                painter.rect_filled(track_rect, 0.0, bg_color);

                painter.line_segment(
                    [
                        egui::pos2(rect.left(), track_rect.top()),
                        egui::pos2(rect.right(), track_rect.top()),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(50)),
                );

                // Vertical grid lines over the lane (at the ruler's major ticks)
                for tick in ruler_ticks.iter().filter(|tick| tick.major) {
                    let x = view.x(rect, tick.secs);
                    painter.line_segment(
                        [egui::pos2(x, track_rect.top()), egui::pos2(x, track_rect.bottom())],
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(42)),
                    );
                }

                let lane_layout = take_lane_layout(&self.project, audio_clips);
                for clip_id in audio_clips {
                    if let Some(clip) = self.project.audio_clips.get(*clip_id) {
                        let (index, count) =
                            lane_layout.get(clip_id).copied().unwrap_or((0, 1));
                        let lane = if count <= 1 {
                            track_rect
                        } else {
                            let lane_height = track_rect.height() / count as f32;
                            egui::Rect::from_min_max(
                                egui::pos2(
                                    track_rect.left(),
                                    track_rect.top() + index as f32 * lane_height,
                                ),
                                egui::pos2(
                                    track_rect.right(),
                                    track_rect.top() + (index as f32 + 1.0) * lane_height,
                                ),
                            )
                        };
                        let clip_rect = draw_audio_clip(
                            &painter,
                            &mut self.detail_cache,
                            clip,
                            lane,
                            *color,
                            view,
                            project_sr,
                            self.selected_clips.contains(clip_id),
                            count > 1,
                        );
                        if clip_rect != egui::Rect::NOTHING {
                            clip_rects.push((*clip_id, clip_rect));
                        }
                    }
                }

                // Separators between take sub-lanes.
                if let Some(count) = lane_layout.values().map(|(_, count)| *count).max() {
                    if count > 1 {
                        for i in 1..count {
                            let y = track_rect.top()
                                + track_rect.height() * i as f32 / count as f32;
                            painter.line_segment(
                                [
                                    egui::pos2(track_rect.left(), y),
                                    egui::pos2(track_rect.right(), y),
                                ],
                                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(62)),
                            );
                        }
                    }
                }

                for clip_id in midi_clips {
                    if let Some(clip) = self.project.midi_clips.get(*clip_id) {
                        let clip_rect = draw_midi_clip(
                            &painter,
                            clip,
                            track_rect,
                            *color,
                            view,
                            project_sr,
                            &self.project.tempo_map,
                            self.selected_midi_clips.contains(clip_id),
                        );
                        if clip_rect != egui::Rect::NOTHING {
                            midi_clip_rects.push((*clip_id, clip_rect));
                        }
                    }
                }

                // ---- Automatic crossfade overlay ----
                // The mixer equal-power crossfades overlapping unmuted clips
                // on the same track; show where that happens.
                let mut spans: Vec<(f64, f64)> = Vec::new();
                for clip_id in audio_clips {
                    if let Some(clip) = self.project.audio_clips.get(*clip_id) {
                        if clip.muted || clip.length == 0 {
                            continue;
                        }
                        let start = clip.timeline_position.sample as f64 / project_sr;
                        let end = start
                            + clip.length as f64 / clip.source.sample_rate.max(1) as f64;
                        spans.push((start, end));
                    }
                }
                for i in 0..spans.len() {
                    for j in (i + 1)..spans.len() {
                        // Match the mixer: only clips that partially overlap
                        // get an automatic crossfade.
                        let a_contains_b = spans[i].0 <= spans[j].0 && spans[i].1 >= spans[j].1;
                        let b_contains_a = spans[j].0 <= spans[i].0 && spans[j].1 >= spans[i].1;
                        if a_contains_b || b_contains_a {
                            continue;
                        }
                        let start = spans[i].0.max(spans[j].0);
                        let end = spans[i].1.min(spans[j].1);
                        if end <= start {
                            continue;
                        }
                        let x0 = view.x(rect, start);
                        let x1 = view.x(rect, end);
                        if x1 < rect.left() || x0 > rect.right() {
                            continue;
                        }
                        let top = track_rect.top() + 18.0;
                        let bottom = track_rect.bottom() - 6.0;
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(x0.max(rect.left()), top),
                                egui::pos2(x1.min(rect.right()), bottom),
                            ),
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(255, 214, 120, 18),
                        );
                        let stroke = egui::Stroke::new(
                            1.5_f32,
                            egui::Color32::from_rgba_unmultiplied(255, 214, 120, 90),
                        );
                        painter.line_segment(
                            [egui::pos2(x0, top), egui::pos2(x1, bottom)],
                            stroke,
                        );
                        painter.line_segment(
                            [egui::pos2(x0, bottom), egui::pos2(x1, top)],
                            stroke,
                        );
                    }
                }
            }

            // ---- Clip selection (click) ----
            let (primary_clicked, pointer_pos, press_origin) = ui.input(|i| {
                (
                    i.pointer.primary_clicked(),
                    i.pointer.hover_pos(),
                    i.pointer.press_origin(),
                )
            });
            let additive = ui.input(|input| {
                input.modifiers.command
                    || input.modifiers.ctrl
                    || input.modifiers.shift
            });
            if primary_clicked && hovered {
                if let Some(pos) = pointer_pos {
                    let clicked_midi = midi_clip_rects
                        .iter()
                        .rev()
                        .find(|(_, rect)| rect.contains(pos))
                        .map(|(id, _)| *id);
                    let clicked_audio = clip_rects
                        .iter()
                        .rev()
                        .find(|(_, rect)| rect.contains(pos))
                        .map(|(id, _)| *id);
                    if let Some(midi_id) = clicked_midi {
                        if additive {
                            self.toggle_midi_selection(midi_id);
                        } else {
                            self.set_single_midi_selection(midi_id);
                        }
                    } else if let Some(clip_id) = clicked_audio {
                        if additive {
                            self.toggle_selection(clip_id);
                        } else {
                            self.set_single_selection(clip_id);
                        }
                    } else if pos.y > rect.top() + ruler_height && !additive {
                        self.clear_selection();
                    }
                }
            }

            // ---- Clip editing: drag body to move, edges to trim, corners to fade ----
            let edit_area = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + ruler_height),
                rect.max,
            );
            let edit_response = ui.interact(
                edit_area,
                ui.id().with("clip_edit"),
                egui::Sense::click_and_drag(),
            );
            if edit_response.double_clicked() {
                if let Some(pos) = pointer_pos {
                    if let Some((midi_id, _)) = midi_clip_rects
                        .iter()
                        .rev()
                        .find(|(_, rect)| rect.contains(pos))
                    {
                        self.set_single_midi_selection(*midi_id);
                        self.piano_roll = Some(PianoRollWindow::new(*midi_id, self.snap_mode));
                    }
                }
            }
            // Right-clicking an audio clip reopens the clip audio-tools window
            // (reverse, pitch, time stretch, etc.). Track effects live in the
            // FX Rack and are intentionally not part of this menu.
            if edit_response.secondary_clicked() {
                if let Some(pos) = pointer_pos {
                    if let Some((clip_id, _)) = Self::clip_hit(&clip_rects, pos) {
                        if !self.selected_clips.contains(&clip_id) {
                            self.set_single_selection(clip_id);
                        } else {
                            self.selected_clip = Some(clip_id);
                        }
                        let ids = self.selection_ids();
                        self.audio_window = Some(ClipAudioWindow::new(ids));
                    }
                }
            }
            if edit_response.drag_started_by(egui::PointerButton::Primary) {
                // Classify the gesture from where the pointer went down, not
                // from where it is after egui's drag threshold: otherwise edge
                // grabs would already be inside the body and become moves.
                if let Some(pos) = press_origin.or(pointer_pos) {
                    let midi_hit = Self::midi_clip_hit(&midi_clip_rects, pos);
                    if midi_hit.is_some() {
                        self.begin_midi_clip_drag(pos, &midi_clip_rects, rect, view);
                    } else if Self::clip_hit(&clip_rects, pos).is_some() {
                        self.begin_clip_drag(pos, &clip_rects, rect, view);
                    } else if pos.y > rect.top() + ruler_height {
                        // Empty track space: rubber-band selection.
                        self.marquee = Some((pos, pos));
                        self.marquee_additive = additive;
                        if !additive {
                            self.clear_selection();
                        }
                    }
                }
            }
            if edit_response.dragged_by(egui::PointerButton::Primary) {
                if self.midi_drag.is_some() {
                    if let Some(pos) = pointer_pos {
                        self.update_midi_clip_drag(
                            pos,
                            rect,
                            track_y_start,
                            track_height,
                            view,
                        );
                    }
                } else if let Some((start, _)) = self.marquee {
                    if let Some(pos) = pointer_pos {
                        self.marquee = Some((start, pos));
                    }
                } else if let Some(pos) = pointer_pos {
                    self.update_clip_drag(pos, rect, track_y_start, track_height, view);
                }
            }
            if edit_response.drag_stopped_by(egui::PointerButton::Primary) {
                if let Some((start, end)) = self.marquee.take() {
                    let marquee = egui::Rect::from_two_pos(start, end);
                    let audio_hits = clips_in_marquee(&clip_rects, marquee);
                    let midi_hits = midi_clips_in_marquee(&midi_clip_rects, marquee);
                    if self.marquee_additive {
                        for clip_id in audio_hits {
                            self.add_to_selection(clip_id);
                        }
                        for clip_id in midi_hits {
                            self.add_to_midi_selection(clip_id);
                        }
                    } else {
                        self.selected_clips = audio_hits.iter().copied().collect();
                        self.selected_clip = audio_hits.last().copied();
                        self.selected_midi_clips =
                            midi_hits.iter().copied().collect();
                        self.selected_midi_clip = midi_hits.last().copied();
                    }
                    self.marquee_additive = false;
                } else if self.midi_drag.is_some() {
                    self.finish_midi_clip_drag();
                } else {
                    self.finish_clip_drag();
                }
            }

            // Marquee overlay
            if let Some((start, end)) = self.marquee {
                let painter = ui.painter();
                let marquee = egui::Rect::from_two_pos(start, end);
                painter.rect_filled(
                    marquee,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 36),
                );
                painter.rect_stroke(
                    marquee,
                    0.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 170, 240)),
                );
            }

            // Hover feedback: resize cursor over trim/fade zones, grab over bodies.
            if self.drag.is_none() && self.midi_drag.is_none() && !self.dragging_playhead && hovered {
                if let Some(pos) = pointer_pos {
                    if let Some((_, mode)) = Self::midi_clip_hit(&midi_clip_rects, pos) {
                        let icon = match mode {
                            DragMode::Move => egui::CursorIcon::Grab,
                            _ => egui::CursorIcon::ResizeHorizontal,
                        };
                        ui.output_mut(|o| o.cursor_icon = icon);
                    } else if let Some((_, mode)) = Self::clip_hit(&clip_rects, pos) {
                        let icon = match mode {
                            DragMode::Move => egui::CursorIcon::Grab,
                            _ => egui::CursorIcon::ResizeHorizontal,
                        };
                        ui.output_mut(|o| o.cursor_icon = icon);
                    }
                }
            }

            // ---- Playhead ----
            let duration = self.timeline_duration();
            let playhead_x = view.x(rect, self.playhead_secs);

            painter.line_segment(
                [egui::pos2(playhead_x, rect.top()), egui::pos2(playhead_x, rect.bottom())],
                egui::Stroke::new(2.0_f32, egui::Color32::RED),
            );

            let triangle_size = 8.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(playhead_x - triangle_size, rect.top()),
                    egui::pos2(playhead_x + triangle_size, rect.top()),
                    egui::pos2(playhead_x, rect.top() + triangle_size),
                ],
                egui::Color32::RED,
                egui::Stroke::NONE,
            ));

            // Playhead drag interaction
            let playhead_rect = egui::Rect::from_min_max(
                egui::pos2(playhead_x - 6.0, rect.top()),
                egui::pos2(playhead_x + 6.0, rect.bottom()),
            );
            let response = ui.interact(
                playhead_rect,
                ui.id().with("playhead"),
                egui::Sense::drag(),
            );
            if response.dragged_by(egui::PointerButton::Primary) {
                if !self.dragging_playhead {
                    self.dragging_playhead = true;
                    // Seeking stops the transport (and ends an active take).
                    if self.playing || self.recording {
                        self.pause_transport();
                    }
                }
                if let Some(pos) = pointer_pos {
                    // Allow placing the playhead anywhere on screen, including
                    // past the last clip.
                    let max_t = duration.max(view.t(rect, rect.right()));
                    self.playhead_secs = view.t(rect, pos.x).clamp(0.0, max_t);
                }
            } else if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.dragging_playhead = false;
            }

            if ui.rect_contains_pointer(playhead_rect) || self.dragging_playhead {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            }
        });

        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let status = if self.recording {
                    "● RECORDING"
                } else if self.pending_record_track.is_some() {
                    "● REC (waiting for play)"
                } else if self.playing {
                    "Playing"
                } else {
                    "Stopped"
                };
                ui.label(status);
                ui.separator();
                let sel = if self.selected_clips.len() + self.selected_midi_clips.len() > 1 {
                    format!(
                        "{} clips",
                        self.selected_clips.len() + self.selected_midi_clips.len()
                    )
                } else if !self.selected_midi_clips.is_empty() {
                    "MIDI clip".to_string()
                } else {
                    self.selected_clip
                        .and_then(|id| self.project.audio_clips.get(id))
                        .and_then(|clip| clip.source.path.file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or("none")
                        .to_string()
                };
                ui.label(format!("Selected: {}", sel));
                ui.separator();
                ui.label(format!(
                    "Zoom: {:.2} px/s",
                    self.pixels_per_sec
                ));
                ui.separator();
                ui.label("Zoft DAW v0.1.0");
            });
        });

        self.show_audio_window(ctx);
        self.show_fx_rack(ctx);
        self.show_piano_roll(ctx);
        self.show_history_window(ctx);

        // Keep the event loop waking regularly, even when the window is not
        // focused, so the compositor never marks the app unresponsive.
        if self.playing || self.recording {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

impl AppUI {
    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key;

        // Only handle keyboard if no text input is focused
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }

        let modifiers = ctx.input(|i| i.modifiers);
        let command = modifiers.command || modifiers.ctrl;

        if command {
            // egui-winit turns Ctrl+C/X/V into Clipboard events instead of key
            // presses, so listen for those as well as ordinary key events.
            let (
                undo_pressed,
                redo_shift_pressed,
                redo_y_pressed,
                copy_pressed,
                cut_pressed,
                paste_pressed,
                paste_released,
                duplicate_pressed,
                save_pressed,
                save_as_pressed,
                open_pressed,
            ) = ctx.input(|i| {
                let has_copy = i
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy));
                let has_cut = i
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Cut));
                let has_paste = i
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Paste(_)));

                let z_pressed = i.key_pressed(Key::Z);
                let s_pressed = i.key_pressed(Key::S);
                (
                    z_pressed,
                    z_pressed && i.modifiers.shift,
                    i.key_pressed(Key::Y),
                    has_copy || i.key_pressed(Key::C),
                    has_cut || i.key_pressed(Key::X),
                    has_paste || i.key_pressed(Key::V),
                    (i.modifiers.command || i.modifiers.ctrl) && i.key_released(Key::V),
                    i.key_pressed(Key::D),
                    s_pressed,
                    s_pressed && i.modifiers.shift,
                    i.key_pressed(Key::O),
                )
            });

            if copy_pressed {
                self.copy_selected_clip();
            }
            if cut_pressed {
                self.copy_selected_clip();
                self.delete_selected_clip();
            }
            if paste_pressed {
                self.paste_clip();
                self.last_paste_signal = Some(std::time::Instant::now());
            } else if paste_released
                && self
                    .last_paste_signal
                    .map(|instant| instant.elapsed().as_millis() >= 500)
                    .unwrap_or(true)
            {
                // Some backends emit no Paste event when the system clipboard
                // is empty; the key release still arrives, so use it.
                self.paste_clip();
            }

            if duplicate_pressed {
                self.duplicate_selected_clip();
            }
            if save_pressed {
                self.handle_file_action(if save_as_pressed {
                    FileAction::SaveAs
                } else {
                    FileAction::Save
                });
            }
            if open_pressed {
                self.handle_file_action(FileAction::Open);
            }

            if undo_pressed {
                if redo_shift_pressed {
                    self.redo();
                } else {
                    self.undo();
                }
            }
            if redo_y_pressed {
                self.redo();
            }
            return;
        }

        // Space: Play/Pause (toggle) - resumes from current position
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_playback();
        }

        // Enter: Return to start (stops the transport and any take)
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.pause_transport();
            self.playhead_secs = 0.0;
        }

        // Home / End: jump to the start or end of the project.
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.pause_transport();
            self.playhead_secs = 0.0;
        }
        if ctx.input(|i| i.key_pressed(Key::End)) {
            self.pause_transport();
            self.playhead_secs = self.timeline_duration();
        }

        // R: Record (toggle recording)
        if ctx.input(|i| i.key_pressed(Key::R)) {
            self.toggle_recording();
        }

        // S: split the selected clip at the playhead
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.split_selected_clip();
        }

        // Shift+M: add a marker at the playhead. M: mute/unmute the selected
        // clips.
        if ctx.input(|i| i.modifiers.shift && i.key_pressed(Key::M)) {
            self.add_marker_at_playhead();
        } else if ctx.input(|i| i.key_pressed(Key::M)) {
            self.toggle_selected_clips_muted();
        }

        // X: show or hide the FX Rack
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::X)) {
            if self.fx_rack_open {
                self.commit_pending_insert_edit();
            }
            self.fx_rack_open = !self.fx_rack_open;
        }

        // Delete/Backspace: remove the selected clip (recoverable via undo).
        // While the piano roll is open, its grid handles Delete for notes.
        if self.piano_roll.is_none()
            && ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
        {
            self.delete_selected_clip();
        }

        // +/-: zoom, F: fit
        if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
            self.zoom_center(1.5);
        }
        if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.zoom_center(1.0 / 1.5);
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.needs_fit = true;
        }
    }

    fn toggle_playback(&mut self) {
        if self.playing {
            self.pause_transport();
        } else {
            self.playing = true;
            self.start_playback();
            self.last_playhead_update = std::time::Instant::now();

            // An armed take either starts now or waits for the punch-in point.
            if let Some(track_id) = self.pending_record_track {
                if punch_waiting(
                    self.punch_enabled,
                    self.punch_in_secs,
                    self.punch_out_secs,
                    self.playhead_secs,
                ) {
                    // The playback poll starts the take at punch-in.
                } else {
                    self.pending_record_track = None;
                    self.begin_recording(track_id);
                }
            }
        }
    }

    /// Stop the transport. If a take is in progress it is ended here, so the
    /// recorded waveform appears as soon as the transport is paused instead of
    /// only when the record button is toggled off.
    fn pause_transport(&mut self) {
        self.playing = false;
        // Invalidate any in-flight background reload.
        self.playback_generation = self.playback_generation.wrapping_add(1);
        self.engine.clear_playback();
        if self.recording {
            self.stop_recording_and_create_clip();
        }
        let _ = self.engine.stop();
    }

    /// Push the current loop range into the engine. Called whenever the loop
    /// controls change and before playback starts.
    fn sync_playback_loop(&self) {
        let out_rate = self.engine.output_sample_rate().max(1);
        self.engine.set_playback_loop(loop_range_frames(
            self.loop_enabled,
            self.loop_start_secs,
            self.loop_end_secs,
            out_rate,
        ));
    }

    /// Push the current metronome settings into the engine.
    fn sync_playback_metronome(&self) {
        let out_rate = self.engine.output_sample_rate().max(1);
        let (numerator, denominator) = self.time_signature();
        let bpm = self.project.tempo_map.default_bpm();
        self.engine.set_playback_metronome(metronome_config(
            self.metronome_enabled,
            bpm,
            numerator,
            denominator,
            out_rate,
        ));
    }

    /// Register a tap and update the project tempo from the recent intervals.
    fn tap_tempo(&mut self) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_tap {
            let delta = now.duration_since(last).as_secs_f64();
            if (0.05..2.0).contains(&delta) {
                self.tap_intervals.push_back(delta);
                while self.tap_intervals.len() > 4 {
                    self.tap_intervals.pop_front();
                }
            } else {
                // A long pause starts a new tap sequence.
                self.tap_intervals.clear();
            }
        }
        self.last_tap = Some(now);

        let intervals: Vec<f64> = self.tap_intervals.iter().copied().collect();
        if let Some(bpm) = bpm_from_tap_intervals(&intervals) {
            self.commit_tempo(bpm);
        }
    }

    /// Set the project tempo and record one undoable entry.
    fn commit_tempo(&mut self, bpm: f32) {
        let from = self.project.tempo_map.default_bpm();
        if (bpm - from).abs() < 1e-4 {
            return;
        }
        self.project.tempo_map.set_default_bpm(bpm);
        self.push_undo(EditAction::Tempo { from, to: bpm }, false);
        self.after_edit();
        self.sync_playback_metronome();
    }

    /// Handle an edit to the loop controls: keep the range valid, move the
    /// playhead inside it, and restart the mixer if the transport is running.
    fn loop_changed(&mut self) {
        if self.loop_enabled && self.loop_end_secs <= self.loop_start_secs {
            // Enabling Loop with no range loops the project (or at least 4 s).
            self.loop_start_secs = 0.0;
            self.loop_end_secs = self.timeline_duration().max(4.0);
        }
        if self.loop_enabled
            && (self.playhead_secs < self.loop_start_secs
                || self.playhead_secs >= self.loop_end_secs)
        {
            self.playhead_secs = self.loop_start_secs.max(0.0);
        }

        self.sync_playback_loop();
        if self.playing {
            // Rebuild the mixer asynchronously so this edit doesn't block the
            // UI thread while the transport is rolling.
            self.request_playback_reload();
        }
    }

    /// Load the project's audio clips into the engine and start sound output
    /// from the current playhead position.
    fn start_playback(&mut self) {
        let out_rate = self.engine.output_sample_rate().max(1);
        let start_frame = (self.playhead_secs.max(0.0) * out_rate as f64) as u64;
        self.playback_generation = self.playback_generation.wrapping_add(1);
        self.sync_playback_loop();
        self.sync_playback_metronome();
        // Open the output immediately with silence; the worker supplies the
        // clips as soon as they have been read from disk.
        self.engine.set_playback_notes(Vec::new());
        self.engine.set_playback_tracks(Vec::new());
        self.engine.set_playback_clips(Vec::new(), start_frame);
        if let Err(e) = self.engine.start() {
            self.engine.clear_playback();
            self.playing = false;
            self.error_message = Some(format!("Failed to start playback: {}", e));
            return;
        }
        self.request_playback_reload();
    }

    /// Read the audio clips that are placed on tracks into playable buffers.
    /// Timeline positions are converted from project samples to output-device
    /// frames, and each buffer is sliced to the clip's trimmed source range.
    /// Describe the audio clips that should feed the mixer. Timeline positions
    /// are converted from project samples to output-device frames; the sources
    /// are read on a worker thread.
    fn collect_playback_specs(&self, out_rate: u32) -> Vec<PlaybackClipSpec> {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let any_solo = self.project.tracks.values().any(|track| track.solo);
        let mut specs = Vec::new();

        for (track_index, track_id) in self.project.track_order.iter().enumerate() {
            let Some(track) = self.project.tracks.get(track_id) else {
                continue;
            };
            if audible_track_gain(track, any_solo).is_none() {
                continue;
            }
            for clip_id in &track.clips {
                let Some(clip) = self.project.audio_clips.get(*clip_id) else {
                    continue;
                };
                if clip.muted || clip.length == 0 {
                    continue;
                }
                let start_frame = (clip.timeline_position.sample as f64
                    * out_rate as f64
                    / project_sr)
                    .round() as u64;
                specs.push(PlaybackClipSpec {
                    path: clip.source.path.clone(),
                    source_offset: clip.source_offset,
                    length: clip.length,
                    start_frame,
                    track: track_index as u32,
                    gain: clip.gain,
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                });
            }
        }

        specs
    }

    /// Describe every track's fader gain and insert-effect chain. Indices match
    /// the `track` field on playback clips and MIDI notes.
    fn collect_playable_tracks(&self) -> Vec<PlayableTrack> {
        let any_solo = self.project.tracks.values().any(|track| track.solo);
        self.project
            .track_order
            .iter()
            .enumerate()
            .filter_map(|(index, track_id)| {
                self.project.tracks.get(track_id).map(|track| PlayableTrack {
                    index: index as u32,
                    gain: audible_track_gain(track, any_solo).unwrap_or(0.0),
                    pan_left: track.pan.left_gain,
                    pan_right: track.pan.right_gain,
                    effects: track
                        .inserts
                        .iter()
                        .filter_map(PlayableEffect::from_slot)
                        .collect(),
                })
            })
            .collect()
    }

    /// Ask the worker to rebuild the playback clips from disk. Until the
    /// result arrives the current mixer keeps playing, so the UI never blocks
    /// on WAV I/O while the transport is rolling.
    fn request_playback_reload(&mut self) {
        if !self.playing {
            return;
        }
        self.playback_generation = self.playback_generation.wrapping_add(1);
        let out_rate = self.engine.output_sample_rate().max(1);
        let specs = self.collect_playback_specs(out_rate);
        let _ = self
            .playback_spec_tx
            .send((self.playback_generation, specs));
    }

    /// Pair note-on/off events on every audible MIDI clip into playable notes.
    /// Clip event ticks are relative to the clip; convert through the tempo map
    /// into output-device frames.
    fn collect_playable_notes(&self, out_rate: u32) -> Vec<PlayableNote> {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let out_rate = out_rate.max(1) as f64;
        let any_solo = self.project.tracks.values().any(|track| track.solo);
        let mut notes = Vec::new();

        let to_frame = |tick: u64| -> u64 {
            let sample = self.project.tempo_map.tick_to_sample(tick, project_sr);
            (sample as f64 * out_rate / project_sr).round() as u64
        };
        let seek_tick = self.project.tempo_map.sample_to_tick(
            (self.playhead_secs.max(0.0) * project_sr).round() as u64,
            project_sr,
        );

        for (track_index, track_id) in self.project.track_order.iter().enumerate() {
            let Some(track) = self.project.tracks.get(track_id) else {
                continue;
            };
            if audible_track_gain(track, any_solo).is_none() {
                continue;
            }
            for clip_id in &track.midi_clips {
                let Some(clip) = self.project.midi_clips.get(*clip_id) else {
                    continue;
                };
                if clip.muted || clip.length == 0 {
                    continue;
                }
                let clip_start = clip.timeline_position.tick;
                let content_seek = (seek_tick as i64
                    - clip.timeline_position.tick as i64
                    + clip.content_offset as i64)
                    .max(0) as u64;
                let cc_pan = midi_clip_cc_points(clip, 10);
                let cc_volume = midi_clip_cc_points(clip, 7);
                let cc_expression = midi_clip_cc_points(clip, 11);
                for (start, end, note, velocity) in midi_clip_note_spans(clip) {
                    let content_tick = clip.content_offset.saturating_add(start);
                    let sample_tick = content_tick.max(content_seek);
                    let pan_value = midi_cc_value_at(&cc_pan, sample_tick, 64);
                    let volume = midi_cc_value_at(&cc_volume, sample_tick, 127);
                    let expression =
                        midi_cc_value_at(&cc_expression, sample_tick, 127);
                    let gain = (volume as f32 / 127.0)
                        * (expression as f32 / 127.0);
                    let (pan_left, pan_right) = cc_pan_gains(pan_value);
                    notes.push(playable_note(
                        clip_start.saturating_add(start),
                        clip_start.saturating_add(end),
                        note,
                        velocity,
                        track_index as u32,
                        pan_left,
                        pan_right,
                        gain,
                        &to_frame,
                    ));
                }
            }
        }

        notes.sort_by_key(|note| (note.start_frame, note.note));
        notes
    }

    /// Apply finished background playback reloads. Stale generations are
    /// discarded.
    fn poll_playback_reload(&mut self) {
        while let Ok((generation, clips)) = self.playback_ready_rx.try_recv() {
            if generation != self.playback_generation || !self.playing {
                continue;
            }
            let out_rate = self.engine.output_sample_rate().max(1);
            let start_frame = (self.playhead_secs.max(0.0) * out_rate as f64) as u64;
            self.sync_playback_loop();
            self.sync_playback_metronome();
            let tracks = self.collect_playable_tracks();
            self.engine.set_playback_tracks(tracks);
            let notes = self.collect_playable_notes(out_rate);
            self.engine.set_playback_notes(notes);
            self.engine.set_playback_clips(clips, start_frame);
        }
    }

    fn toggle_recording(&mut self) {
        if self.recording {
            self.stop_recording_and_create_clip();
        } else if self.pending_record_track.is_some() {
            // Cancel an armed take that never rolled.
            self.pending_record_track = None;
        } else {
            // Find all armed tracks
            let armed_tracks: Vec<TrackId> = self
                .project
                .track_order
                .iter()
                .filter(|id| {
                    self.project
                        .tracks
                        .get(*id)
                        .map(|t| t.record_arm)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();

            if armed_tracks.is_empty() {
                self.error_message =
                    Some("No tracks are armed for recording. Click the ● button first.".to_string());
                return;
            }

            // Record on the first armed track.
            let track_id = armed_tracks[0];
            if self.playing {
                // Already rolling: start now, or wait for the punch-in point.
                if punch_waiting(
                    self.punch_enabled,
                    self.punch_in_secs,
                    self.punch_out_secs,
                    self.playhead_secs,
                ) {
                    self.pending_record_track = Some(track_id);
                    self.error_message = None;
                } else {
                    self.begin_recording(track_id);
                }
            } else {
                // Wait for the transport to roll before capturing audio, so the
                // take starts exactly where the playhead starts moving.
                self.pending_record_track = Some(track_id);
                self.error_message = None;
            }
        }
    }

    /// Check free space for the recording directory and build a warning string
    /// when less than 10 minutes remain.
    fn low_disk_warning(&self) -> Option<String> {
        let seconds = self.engine.available_recording_seconds(Path::new("."))?;
        if seconds < 600.0 {
            Some(format!(
                "Low disk space: about {:.0} min of recording left.",
                seconds / 60.0
            ))
        } else {
            None
        }
    }

    fn handle_file_action(&mut self, action: FileAction) {
        match action {
            FileAction::New => self.new_project(),
            FileAction::NewTemplate(template) => self.new_project_from_template(template),
            FileAction::Open => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Zoft project", &["zoft"])
                    .pick_file()
                {
                    self.load_project(&path);
                }
            }
            FileAction::OpenPath(path) => self.load_project(&path),
            FileAction::Save => {
                if let Some(path) = self.project.project_path.clone() {
                    self.save_project_to(&path);
                } else {
                    self.save_project_as();
                }
            }
            FileAction::SaveAs => self.save_project_as(),
            FileAction::ClearRecent => {
                self.recent_projects.clear();
                self.save_recent_projects();
            }
        }
    }

    fn recent_projects_file() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("zoft").join("recent_projects.txt"))
    }

    fn load_recent_projects() -> Vec<PathBuf> {
        let Some(path) = Self::recent_projects_file() else {
            return Vec::new();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .take(10)
            .collect()
    }

    fn save_recent_projects(&self) {
        let Some(path) = Self::recent_projects_file() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = self
            .recent_projects
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(path, text);
    }

    fn remember_project(&mut self, path: &Path) {
        self.recent_projects.retain(|existing| existing != path);
        self.recent_projects.insert(0, path.to_path_buf());
        self.recent_projects.truncate(10);
        self.save_recent_projects();
    }

    fn sibling_autosave(path: &Path) -> PathBuf {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("project");
        path.with_file_name(format!("{}.autosave.zoft", stem))
    }

    fn autosave_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.project.project_path {
            return Some(Self::sibling_autosave(path));
        }
        dirs::data_dir().map(|dir| {
            dir.join("zoft")
                .join("autosave")
                .join(format!("{}.zoft", self.project.id))
        })
    }

    fn autosave_project(&mut self) {
        let Some(path) = self.autosave_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match ProjectSerialization::save(&self.project, &path) {
            Ok(()) => self.autosave_status = Some("Autosaved just now".to_string()),
            Err(e) => self.autosave_status = Some(format!("Autosave failed: {}", e)),
        }
    }

    fn load_recovery(&mut self, path: &Path) {
        match ProjectSerialization::load(path) {
            Ok(mut project) => {
                // Keep the recovered state, but require an explicit Save As
                // for the original project file.
                project.project_path = None;
                self.adopt_project(project);
                self.recovery_offer = None;
                self.error_message = None;
            }
            Err(e) => self.error_message = Some(format!("Could not load recovery: {}", e)),
        }
    }

    fn save_project_as(&mut self) {
        let file_name = format!("{}.zoft", self.project.name);
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("Zoft project", &["zoft"])
            .set_file_name(&file_name)
            .save_file()
        {
            if path.extension().and_then(|ext| ext.to_str()) != Some("zoft") {
                path.set_extension("zoft");
            }
            self.save_project_to(&path);
        }
    }

    fn save_project_to(&mut self, path: &Path) {
        // The project name tracks the file name (without the extension).
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            self.project.name = stem.to_string();
        }
        match ProjectSerialization::save(&self.project, path) {
            Ok(()) => {
                self.project.project_path = Some(path.to_path_buf());
                self.remember_project(path);
                // The project file is now the newest copy; drop its autosave.
                let _ = std::fs::remove_file(Self::sibling_autosave(path));
                self.recovery_offer = None;
                self.error_message = None;
            }
            Err(e) => self.error_message = Some(format!("Could not save project: {}", e)),
        }
    }

    fn load_project(&mut self, path: &Path) {
        self.pause_transport();
        match ProjectSerialization::load(path) {
            Ok(project) => {
                self.adopt_project(project);
                // Show the file name (without .zoft) in the UI.
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    self.project.name = stem.to_string();
                }
                self.remember_project(path);
                // Offer a newer autosave for this project, if one exists.
                let autosave = Self::sibling_autosave(path);
                let newer = std::fs::metadata(&autosave)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .zip(
                        std::fs::metadata(path)
                            .and_then(|meta| meta.modified())
                            .ok(),
                    )
                    .map(|(autosave, project)| autosave > project)
                    .unwrap_or(false);
                self.recovery_offer = if newer { Some(autosave) } else { None };
            }
            Err(e) => self.error_message = Some(format!("Could not open project: {}", e)),
        }
    }

    fn new_project(&mut self) {
        self.pause_transport();
        self.adopt_project(Project::new("Untitled"));
    }

    fn new_project_from_template(&mut self, template: Template) {
        self.pause_transport();
        self.adopt_project(project_from_template(template));
    }

    /// Remove a track (and detach its clips into the trash) as one undoable
    /// step. The track object keeps its clip list, so undo restores everything.
    fn remove_track(&mut self, track_id: TrackId) {
        let Some(track) = self.project.tracks.get(&track_id).cloned() else {
            return;
        };
        let index = self
            .project
            .track_order
            .iter()
            .position(|id| *id == track_id)
            .unwrap_or(0);
        let action = EditAction::RemoveTrack { track, index };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.clear_selection();
        self.after_edit();
        self.refresh_playback_if_playing();
        self.sync_monitoring();
    }

    /// Re-scan MIDI inputs, e.g. after plugging a keyboard in.
    fn refresh_midi_inputs(&mut self) {
        self.midi_inputs = daw_engine::list_midi_inputs().unwrap_or_default();
        if let Some(selected) = &self.selected_midi_input {
            if !self.midi_inputs.iter().any(|device| &device.name == selected) {
                self.selected_midi_input = None;
                self.midi_monitor = None;
                self.midi_activity = daw_engine::MidiActivity::default();
            }
        }
    }

    /// Open (or close) the MIDI input monitor for the selected device.
    fn restart_midi_monitor(&mut self) {
        self.midi_monitor = None;
        self.midi_activity = daw_engine::MidiActivity::default();
        let Some(name) = self.selected_midi_input.clone() else {
            return;
        };
        let Some(device) = self.midi_inputs.iter().find(|device| device.name == name) else {
            return;
        };
        match daw_engine::MidiMonitor::start(device.index) {
            Ok(monitor) => self.midi_monitor = Some(monitor),
            Err(e) => self.error_message = Some(format!("Could not open MIDI input: {}", e)),
        }
    }

    /// Re-enumerate MIDI outputs, closing MIDI thru if its port disappeared.
    fn refresh_midi_outputs(&mut self) {
        self.midi_outputs = daw_engine::list_midi_outputs().unwrap_or_default();
        if let Some(selected) = &self.selected_midi_output {
            if !self.midi_outputs.iter().any(|device| &device.name == selected) {
                self.selected_midi_output = None;
                self.midi_thru = None;
                self.midi_thru_enabled = false;
            }
        }
    }

    /// Open (or close) the MIDI thru connection for the selected output.
    fn restart_midi_thru(&mut self) {
        self.midi_thru = None;
        if !self.midi_thru_enabled {
            return;
        }
        let Some(name) = self.selected_midi_output.clone() else {
            return;
        };
        let Some(device) = self.midi_outputs.iter().find(|device| device.name == name) else {
            return;
        };
        match daw_engine::MidiThru::start(device.index) {
            Ok(thru) => self.midi_thru = Some(thru),
            Err(e) => {
                self.error_message =
                    Some(format!("Could not open MIDI output: {}", e));
                self.midi_thru_enabled = false;
            }
        }
    }

    /// Echo one captured input event to the selected MIDI output.
    fn send_midi_thru(&mut self, event: &daw_engine::MidiInputEvent) {
        if !self.midi_thru_enabled {
            return;
        }
        if let Some(thru) = self.midi_thru.as_mut() {
            let _ = thru.send_note(
                event.channel,
                event.note,
                event.velocity,
                event.on,
            );
        }
    }

    /// Add a new audio or MIDI track as one undoable step.
    fn add_track(&mut self, midi: bool) {
        let index = self.project.track_order.len();
        let mut track = if midi {
            Track::new_midi(format!("MIDI {}", index + 1))
        } else {
            Track::new_audio(format!("Audio {}", index + 1))
        };
        track.color = Color::track_color(index);
        let action = EditAction::AddTrack { track };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
    }

    /// Drop a point marker at the playhead.
    fn add_marker_at_playhead(&mut self) {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let sample = (self.playhead_secs.max(0.0) * project_sr) as u64;
        let tick = self
            .project
            .tempo_map
            .sample_to_tick(sample, project_sr);
        let name = format!("Marker {}", self.project.marker_regions.len() + 1);
        let marker = MarkerRegion {
            id: MarkerId::new(),
            name,
            start_tick: tick,
            end_tick: tick,
            color: Color(230, 180, 70, 255),
        };
        let action = EditAction::AddMarker { marker };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
    }

    /// Remove a marker from the ruler.
    fn remove_marker(&mut self, marker: MarkerRegion) {
        let action = EditAction::RemoveMarker { marker };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.after_edit();
    }

    /// Replace the project and reset all UI state that referred to the old one.
    fn adopt_project(&mut self, project: Project) {
        self.project = project;
        self.history.clear();
        self.clear_selection();
        self.deleted_from.clear();
        self.audio_window = None;
        self.fx_rack_open = false;
        self.track_meter_levels.clear();
        self.track_meter_holds.clear();
        self.master_meter_levels = [0.0, 0.0];
        self.pending_insert_edit = None;
        self.piano_roll = None;
        self.piano_roll_preview = None;
        self.clipboard = Vec::new();
        self.midi_clipboard = Vec::new();
        self.drag = None;
        self.midi_drag = None;
        self.track_drag = None;
        self.recovery_offer = None;
        self.playhead_secs = 0.0;
        self.view_start_secs = 0.0;
        self.needs_fit = true;
        self.merge_chain = false;
        self.disk_warning = None;
        self.recording = false;
        self.pending_record_track = None;
        self.playback_generation = self.playback_generation.wrapping_add(1);
        self.engine.set_project_format(
            self.project.sample_rate,
            bit_depth_bits(self.project.bit_depth),
            2,
        );
        self.spawn_missing_overviews();
        self.sync_monitoring();
    }

    /// Open the input device and start capturing a take at the current playhead.
    fn begin_recording(&mut self, track_id: TrackId) {
        let is_midi = self
            .project
            .tracks
            .get(&track_id)
            .map(|track| {
                matches!(
                    track.track_type,
                    TrackType::Midi | TrackType::Instrument
                )
            })
            .unwrap_or(false);
        if is_midi {
            self.begin_midi_recording(track_id);
            return;
        }

        // Unique file per take so new recordings never overwrite older ones.
        let file_path = std::path::PathBuf::from(format!(
            "recording_{}.wav",
            uuid::Uuid::new_v4()
        ));
        self.disk_warning = self.low_disk_warning();
        let start_sample =
            (self.playhead_secs.max(0.0) * self.project.sample_rate.max(1) as f64) as u64;
        self.pending_take_mutes = self.mute_takes_at(track_id, start_sample);
        if let Err(e) = self.engine.start_recording(track_id, file_path) {
            // Undo the mutes if the take never started.
            for action in &self.pending_take_mutes {
                if let EditAction::Shape { clip_id, from, .. } = action {
                    if let Some(clip) = self.project.audio_clips.get_mut(*clip_id) {
                        from.apply(clip);
                    }
                }
            }
            self.pending_take_mutes.clear();
            self.recording = false;
            self.pending_record_track = None;
            self.error_message = Some(format!("Recording failed: {}", e));
        } else {
            self.recording = true;
            self.record_start_secs = Some(self.playhead_secs);
            self.error_message = None;
            // Make the previous take silent immediately if playback is rolling.
            self.refresh_playback_if_playing();
        }
        self.sync_monitoring();
    }

    /// Start capturing note-on/off events from the selected MIDI input.
    fn begin_midi_recording(&mut self, track_id: TrackId) {
        if self.midi_monitor.is_none() {
            self.pending_record_track = None;
            self.error_message =
                Some("No MIDI input selected. Choose one in the MIDI menu.".to_string());
            return;
        }
        // Drop any notes that arrived before the take started.
        if let Some(monitor) = &self.midi_monitor {
            let _ = monitor.drain_events();
        }
        self.recording = true;
        self.recording_midi = true;
        self.midi_record_track = Some(track_id);
        self.record_start_secs = Some(self.playhead_secs);
        self.midi_captured.clear();
        self.midi_record_started_at = Some(std::time::Instant::now());
        self.error_message = None;
        self.sync_monitoring();
    }

    /// Finish a MIDI take and attach the captured events to a new MidiClip.
    fn finalize_midi_recording(&mut self) {
        self.recording = false;
        self.recording_midi = false;
        let Some(track_id) = self.midi_record_track.take() else {
            return;
        };
        if let Some(monitor) = &self.midi_monitor {
            let events = filter_midi_events(
                monitor.drain_events(),
                self.midi_input_channel,
            );
            self.midi_captured.extend(events);
        }
        let events = std::mem::take(&mut self.midi_captured);
        let started_at = self
            .midi_record_started_at
            .take()
            .unwrap_or_else(std::time::Instant::now);
        let start_secs = self.record_start_secs.take().unwrap_or(0.0);
        if events.is_empty() {
            self.error_message = Some("No MIDI notes were captured.".to_string());
            return;
        }
        let end_secs = self.playhead_secs.max(start_secs + 0.05);
        let recorded_secs = started_at.elapsed().as_secs_f64();
        let project_sr = self.project.sample_rate.max(1) as f64;
        let start_sample = (start_secs.max(0.0) * project_sr) as u64;
        let end_sample = (end_secs.max(0.0) * project_sr) as u64;
        let start_tick = self.project.tempo_map.sample_to_tick(start_sample, project_sr);
        let length_ticks = self
            .project
            .tempo_map
            .sample_to_tick(end_sample, project_sr)
            .saturating_sub(start_tick)
            .max(1);
        let mut clip = MidiClip::new(TimelinePos::new(start_tick, start_sample), length_ticks);
        for event in events {
            // Use local arrival time so the first note keeps its real offset
            // from the playhead rather than always starting at tick zero.
            let offset_secs = event
                .received_at
                .saturating_duration_since(started_at)
                .as_secs_f64();
            let sample = start_sample + (offset_secs * project_sr) as u64;
            let tick = self
                .project
                .tempo_map
                .sample_to_tick(sample, project_sr)
                .saturating_sub(start_tick);
            clip.add_event(if event.on {
                MidiEvent::note_on(
                    tick,
                    event.channel,
                    event.note,
                    event.velocity.max(1),
                )
            } else {
                MidiEvent::note_off(tick, event.channel, event.note, 0)
            });
        }
        // Loop recording folds repeated passes into the loop window, so each
        // pass overdubs on top of the previous one instead of extending past
        // the loop end.
        let loop_len_secs = (self.loop_end_secs - self.loop_start_secs).max(0.0);
        if self.loop_enabled
            && loop_len_secs > 0.05
            && recorded_secs >= loop_len_secs * 0.95
        {
            let loop_start_sample =
                (self.loop_start_secs.max(0.0) * project_sr) as u64;
            let loop_end_sample =
                (self.loop_end_secs.max(0.0) * project_sr) as u64;
            let loop_start_tick = self
                .project
                .tempo_map
                .sample_to_tick(loop_start_sample, project_sr);
            let loop_end_tick = self
                .project
                .tempo_map
                .sample_to_tick(loop_end_sample, project_sr);
            let loop_len_ticks = loop_end_tick.saturating_sub(loop_start_tick);
            if loop_len_ticks > 0 {
                clip = fold_midi_clip_to_loop(
                    &clip,
                    loop_start_tick,
                    loop_len_ticks,
                    loop_start_sample,
                );
            }
        }
        if self.input_quantize {
            let (numerator, _) = self.time_signature();
            let grid = PianoRollSnap::from_arrange(self.snap_mode)
                .ticks(numerator.max(1) as u64);
            if grid > 1 {
                quantize_recorded_midi_clip(&mut clip, grid);
            }
        }
        let clip_id = self.project.midi_clips.insert(clip);
        let action = EditAction::AttachMidi {
            track: track_id,
            clip_id,
        };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.set_single_midi_selection(clip_id);
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Stop an active recording and add the finished file to the timeline as an
    /// audio clip (which also queues waveform generation).
    fn stop_recording_and_create_clip(&mut self) {
        if self.recording_midi {
            self.finalize_midi_recording();
            self.sync_monitoring();
            return;
        }
        self.recording = false;
        self.disk_warning = None;
        match self.engine.stop_recording() {
            Ok(Some((track_id, file_path))) => {
                let start_secs = self.record_start_secs.take().unwrap_or(0.0);
                let start_sample =
                    (start_secs.max(0.0) * self.project.sample_rate as f64) as u64;
                self.create_audio_clip(track_id, file_path, start_sample);
            }
            Ok(None) => {}
            Err(e) => self.error_message = Some(format!("Failed to stop recording: {}", e)),
        }
        self.flush_pending_take_mutes();
        self.sync_monitoring();
    }

    fn create_audio_clip(&mut self, track_id: TrackId, file_path: PathBuf, start_sample: u64) {
        let (duration_samples, spec) = match wav_file_info(&file_path) {
            Ok(info) => info,
            Err(e) => {
                self.error_message = Some(format!("Failed to open recorded file: {}", e));
                return;
            }
        };

        if duration_samples == 0 {
            self.error_message = Some(format!(
                "Recorded file {} contains no audio data",
                file_path.display()
            ));
            return;
        }

        let bit_depth = bit_depth_from_spec(spec);

        // Overview is generated in the background so the UI never blocks on I/O.
        let file_ref = AudioFileRef::new(
            file_path.clone(),
            duration_samples,
            spec.sample_rate,
            spec.channels,
            bit_depth,
        );

        let file_ref_id = self.project.audio_pool.add(file_ref);
        let source = match self.project.audio_pool.get(file_ref_id) {
            Some(source) => source.clone(),
            None => return,
        };

        let bpm = self.project.tempo_map.default_bpm();
        let clip = AudioClip::new(
            source,
            TimelinePos::from_sample(start_sample, self.project.sample_rate as f64, bpm),
            duration_samples,
        );

        let project_sr = self.project.sample_rate.max(1) as f64;
        let new_length_out = (duration_samples as f64 * project_sr
            / spec.sample_rate.max(1) as f64)
            .ceil() as u64;
        let new_end = start_sample.saturating_add(new_length_out);

        // Multi-take: mute the covered part of previous takes, but keep the
        // uncovered remainder audible so a short re-take doesn't silence the
        // rest of an existing recording.
        let track_clips = self
            .project
            .tracks
            .get(&track_id)
            .map(|track| track.clips.clone())
            .unwrap_or_default();
        let pending_ids: BTreeSet<ClipId> = self
            .pending_take_mutes
            .iter()
            .filter_map(|action| match action {
                EditAction::Shape { clip_id, .. } => Some(*clip_id),
                _ => None,
            })
            .collect();
        let mut carve_ids = Vec::new();
        for other_id in track_clips {
            let Some(other) = self.project.audio_clips.get(other_id) else {
                continue;
            };
            if other.locked || other.length == 0 {
                continue;
            }
            let (start, end) = clip_span_samples(other, project_sr);
            if start >= new_end || end <= start_sample {
                continue;
            }
            if !pending_ids.contains(&other_id) && other.muted {
                // Respect mutes the user set themselves.
                continue;
            }
            carve_ids.push(other_id);
        }

        self.begin_undo_group("Record take");
        // Record the mutes already applied when the take started.
        for action in std::mem::take(&mut self.pending_take_mutes) {
            self.push_undo(action, false);
        }
        for other_id in carve_ids {
            self.carve_old_take_for_span(other_id, start_sample, new_end);
        }

        let clip_id = self.project.audio_clips.insert(clip);
        let attach = EditAction::Attach {
            track: track_id,
            clip_id,
        };
        self.apply_action(&attach, true);
        self.push_undo(attach, false);
        self.end_undo_group();

        self.set_single_selection(clip_id);
        self.after_edit();
        self.refresh_playback_if_playing();

        self.spawn_overview(clip_id, file_path);
    }
}

/// Draw a MIDI clip block in the arrange lane, including a miniature piano-roll
/// preview of its notes. Returns its screen rectangle.
fn draw_midi_clip(
    painter: &egui::Painter,
    clip: &MidiClip,
    lane: egui::Rect,
    color: egui::Color32,
    view: TimelineView,
    project_sr: f64,
    tempo_map: &TempoMap,
    selected: bool,
) -> egui::Rect {
    let start_sample = tempo_map.tick_to_sample(clip.timeline_position.tick, project_sr);
    let end_sample = tempo_map.tick_to_sample(
        clip.timeline_position.tick.saturating_add(clip.length),
        project_sr,
    );
    let start = start_sample as f64 / project_sr.max(1.0);
    let end = end_sample as f64 / project_sr.max(1.0);
    let x0 = view.x(lane, start);
    let x1 = view.x(lane, end).max(x0 + 1.0);
    if x1 < lane.left() || x0 > lane.right() {
        return egui::Rect::NOTHING;
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(x0.max(lane.left()), lane.top() + 2.0),
        egui::pos2(x1.min(lane.right()), lane.bottom() - 2.0),
    );
    let alpha = if clip.muted {
        35
    } else if selected {
        105
    } else {
        70
    };
    painter.rect_filled(
        rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            alpha,
        ),
    );
    let stroke = if selected {
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE)
    } else {
        egui::Stroke::new(1.0_f32, color)
    };
    painter.rect_stroke(rect, 3.0, stroke);

    let spans = midi_clip_note_spans(clip);
    let note_count = spans.len();
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        format!("MIDI · {} notes", note_count),
        egui::FontId::proportional(9.0),
        egui::Color32::from_gray(230),
    );
    if clip.muted {
        painter.text(
            egui::pos2(rect.right() - 4.0, rect.top() + 2.0),
            egui::Align2::RIGHT_TOP,
            "M",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(230),
        );
    }

    // Miniature note lanes. The preview uses the clip's actual pitch range so
    // melodies remain readable at track height.
    let notes_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.top() + 13.0),
        egui::pos2(rect.right() - 2.0, rect.bottom() - 2.0),
    );
    if clip.length > 0 && notes_rect.height() > 2.0 && !spans.is_empty() {
        let pitch_min = spans.iter().map(|(_, _, note, _)| *note).min().unwrap_or(60);
        let pitch_max = spans.iter().map(|(_, _, note, _)| *note).max().unwrap_or(60);
        let mut low = pitch_min.saturating_sub(2);
        let mut high = pitch_max.saturating_add(2);
        if high.saturating_sub(low) < 12 {
            high = low.saturating_add(12).min(127);
            if high.saturating_sub(low) < 12 {
                low = high.saturating_sub(12);
            }
        }
        let pitch_span = high.saturating_sub(low).max(1) as f32;
        let note_height = (notes_rect.height() / pitch_span).clamp(1.5, 4.0);
        let note_base = egui::Color32::from_rgb(
            color.r().saturating_add(90),
            color.g().saturating_add(90),
            color.b().saturating_add(90),
        );

        for (start_tick, end_tick, note, velocity) in &spans {
            let nx0 = x0 + (*start_tick as f32 / clip.length as f32) * (x1 - x0);
            let nx1 = x0 + (*end_tick as f32 / clip.length as f32) * (x1 - x0);
            if nx1 < rect.left() || nx0 > rect.right() {
                continue;
            }
            let left = nx0.max(notes_rect.left());
            let right = nx1.max(left + 1.0).min(notes_rect.right());
            let y = notes_rect.bottom()
                - ((note.saturating_sub(low)) as f32 + 0.5) / pitch_span
                    * notes_rect.height();
            let note_rect = egui::Rect::from_min_max(
                egui::pos2(left, y - note_height * 0.5),
                egui::pos2(right, y + note_height * 0.5),
            );
            let alpha = 120 + (*velocity as u32 * 135 / 127) as u8;
            painter.rect_filled(
                note_rect,
                1.0,
                egui::Color32::from_rgba_unmultiplied(
                    note_base.r(),
                    note_base.g(),
                    note_base.b(),
                    alpha,
                ),
            );
        }
    }

    rect
}

/// Draw a single audio clip: background, waveform (overview or sample-level),
/// fades, gain label and selection highlight. Returns the full clip rect in
/// screen space (or `Rect::NOTHING` when the clip is invalid).
#[allow(clippy::too_many_arguments)]
fn draw_audio_clip(
    painter: &egui::Painter,
    detail_cache: &mut DetailCache,
    clip: &AudioClip,
    lane: egui::Rect,
    track_color: egui::Color32,
    view: TimelineView,
    project_sr: f64,
    selected: bool,
    stacked: bool,
) -> egui::Rect {
    if clip.length == 0 {
        return egui::Rect::NOTHING;
    }

    let clip_start = clip.timeline_position.sample as f64 / project_sr;
    // Clip duration is exact in source frames; convert with the source sample
    // rate so the drawn length matches the audio that plays.
    let source_sr = clip.source.sample_rate.max(1) as f64;
    let clip_end = clip_start + clip.length as f64 / source_sr;

    let x_start = view.x(lane, clip_start);
    let x_end = view.x(lane, clip_end);
    let width = (x_end - x_start).max(1.0);

    // Clips fill the track lane; a couple of pixels keep the border visible.
    let top = (lane.top() + 2.0).min(lane.bottom() - 2.0);
    let bottom = (lane.bottom() - 2.0).max(top + 2.0);
    let full_rect = egui::Rect::from_min_max(
        egui::pos2(x_start, top),
        egui::pos2(x_start + width, bottom),
    );

    // Cull clips that are completely outside the lane.
    if full_rect.right() < lane.left() || full_rect.left() > lane.right() {
        return full_rect;
    }

    let visible_rect = egui::Rect::from_min_max(
        egui::pos2(full_rect.left().max(lane.left()), full_rect.top()),
        egui::pos2(full_rect.right().min(lane.right()), full_rect.bottom()),
    );

    let base_color = clip
        .color
        .map(|c| egui::Color32::from_rgb(c.0, c.1, c.2))
        .unwrap_or(track_color);

    // Background + border
    let bg_alpha = if selected {
        90
    } else if clip.muted {
        25
    } else {
        55
    };
    painter.rect_filled(
        visible_rect,
        2.0,
        egui::Color32::from_rgba_unmultiplied(
            base_color.r(),
            base_color.g(),
            base_color.b(),
            bg_alpha,
        ),
    );
    let border = if selected {
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE)
    } else {
        egui::Stroke::new(1.0_f32, base_color)
    };
    painter.rect_stroke(visible_rect, 2.0, border);

    // Waveform
    let samples_per_px = source_sr / view.pps.max(f32::EPSILON) as f64;
    let mid_y = visible_rect.center().y;
    let half_height = visible_rect.height() * 0.48;
    let wave_color = if selected {
        base_color
    } else {
        base_color.gamma_multiply(0.85)
    };

    let vis_t_start = view.t(lane, visible_rect.left()).max(clip_start);
    let vis_t_end = view.t(lane, visible_rect.right()).min(clip_end);

    if samples_per_px < DETAIL_TRIGGER_SAMPLES_PER_PX {
        // ---- Sample-accurate detail view ----
        let start_offset = ((vis_t_start - clip_start).max(0.0) * source_sr) as u64;
        let frame_count = (((vis_t_end - vis_t_start).max(0.0) * source_sr).ceil() as usize + 2)
            .min(DETAIL_MAX_FRAMES);
        let start_frame = clip.source_offset.saturating_add(start_offset);

        if let Some(samples) = detail_cache.get(
            &clip.source.path,
            start_frame,
            frame_count,
            clip.source.channels,
            clip.source.sample_rate,
        ) {
            let mut points: Vec<egui::Pos2> = Vec::with_capacity(samples.len());
            for (i, sample) in samples.iter().enumerate() {
                let offset = start_frame.saturating_sub(clip.source_offset) + i as u64;
                let t = clip_start + offset as f64 / source_sr;
                let x = view.x(lane, t);
                if x < lane.left() - 1.0 || x > lane.right() + 1.0 {
                    continue;
                }
                let gain = fade_gain_at(clip, offset as f64) * clip.gain;
                let y = mid_y - (sample * gain).clamp(-1.0, 1.0) * half_height;
                points.push(egui::pos2(x, y));
            }
            if points.len() >= 2 {
                painter.add(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.0_f32, wave_color),
                ));
            }
        }
    } else if let Some(overview) = &clip.source.overview {
        // ---- Min/max overview ----
        let frames_per_entry = overview.frames_per_pixel.max(1) as f64;
        let entries = overview.min_max.len();
        if entries > 0 {
            let px_start = visible_rect.left().floor();
            let px_end = visible_rect.right().ceil();
            let mut x = px_start;

            while x < px_end {
                let t0 = view.t(lane, x);
                let t1 = view.t(lane, x + 1.0);

                let s0 = ((t0 - clip_start).max(0.0) * source_sr)
                    + clip.source_offset as f64;
                let s1 = ((t1 - clip_start).max(0.0) * source_sr)
                    + clip.source_offset as f64;

                let e0 = (s0 / frames_per_entry).floor() as usize;
                let mut e1 = (s1 / frames_per_entry).ceil() as usize;
                if e1 <= e0 {
                    e1 = e0 + 1;
                }
                if e0 >= entries {
                    break;
                }
                let e1 = e1.min(entries);

                let mut min_v = f32::INFINITY;
                let mut max_v = f32::NEG_INFINITY;
                let mut rms_peak = 0.0f32;
                for e in e0..e1 {
                    let (mn, mx) = overview.min_max[e];
                    min_v = min_v.min(mn);
                    max_v = max_v.max(mx);
                    if let Some(rms) = &overview.rms {
                        rms_peak = rms_peak.max(rms[e]);
                    }
                }

                let offset = ((t0 - clip_start).max(0.0) * source_sr) as f64;
                let gain = fade_gain_at(clip, offset) * clip.gain;
                let min_v = (min_v * gain).clamp(-1.0, 1.0);
                let max_v = (max_v * gain).clamp(-1.0, 1.0);

                let peak = max_v.abs().max(min_v.abs());
                let silent = peak < 0.0005;

                if silent {
                    // Silent region detection: dim the column.
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, visible_rect.top()),
                            egui::vec2(1.0, visible_rect.height()),
                        ),
                        0.0,
                        egui::Color32::from_black_alpha(28),
                    );
                } else {
                    painter.line_segment(
                        [
                            egui::pos2(x, mid_y - max_v * half_height),
                            egui::pos2(x, mid_y - min_v * half_height),
                        ],
                        egui::Stroke::new(1.0_f32, wave_color),
                    );

                    // RMS envelope overlay
                    if rms_peak > 0.0005 {
                        let r = (rms_peak * gain).clamp(0.0, 1.0) * half_height;
                        painter.line_segment(
                            [egui::pos2(x, mid_y - r), egui::pos2(x, mid_y + r)],
                            egui::Stroke::new(1.0_f32, wave_color.gamma_multiply(0.55)),
                        );
                    }
                }

                x += 1.0;
            }
        }
    } else {
        // No overview yet (background generation in progress)
        painter.text(
            visible_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Generating waveform…",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(170),
        );
    }

    // ---- Fade curves ----
    draw_fade_curve(painter, clip, true, lane, view, source_sr, clip_start, visible_rect);
    draw_fade_curve(painter, clip, false, lane, view, source_sr, clip_start, visible_rect);

    // Fade handles in the top corners of the clip. Drag them inward to lengthen
    // the fade, outward past the clip edge to remove it. Always visible so the
    // small hit targets are discoverable; brighter when selected.
    if visible_rect.width() > 26.0 {
        let handle = 11.0;
        let handle_color = egui::Color32::from_white_alpha(if selected { 220 } else { 110 });
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(visible_rect.left(), visible_rect.top()),
                egui::pos2(visible_rect.left() + handle, visible_rect.top()),
                egui::pos2(visible_rect.left(), visible_rect.top() + handle),
            ],
            handle_color,
            egui::Stroke::NONE,
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(visible_rect.right() - handle, visible_rect.top()),
                egui::pos2(visible_rect.right(), visible_rect.top()),
                egui::pos2(visible_rect.right(), visible_rect.top() + handle),
            ],
            handle_color,
            egui::Stroke::NONE,
        ));
    }

    if !stacked {
        let name = clip
            .source
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("clip");
        // A translucent strip keeps the label readable over the waveform.
        painter.rect_filled(
            egui::Rect::from_min_max(
                visible_rect.left_top(),
                egui::pos2(
                    (visible_rect.left() + 110.0).min(visible_rect.right()),
                    visible_rect.top() + 12.0,
                ),
            ),
            2.0,
            egui::Color32::from_black_alpha(70),
        );
        painter.text(
            egui::pos2(visible_rect.left() + 4.0, visible_rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::proportional(9.0),
            egui::Color32::from_gray(220),
        );
        if (clip.gain - 1.0).abs() > 0.001 {
            painter.text(
                egui::pos2(visible_rect.left() + 4.0, visible_rect.top() + 12.0),
                egui::Align2::LEFT_TOP,
                format!("{:+.1} dB", 20.0 * clip.gain.max(1e-6).log10()),
                egui::FontId::proportional(8.0),
                egui::Color32::from_gray(200),
            );
        }
    }

    if clip.muted {
        painter.rect_filled(
            full_rect,
            3.0,
            egui::Color32::from_rgba_unmultiplied(15, 15, 20, 140),
        );
        painter.text(
            egui::pos2(visible_rect.left() + 4.0, visible_rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            "M",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(230),
        );
    }

    full_rect
}

/// Draw the fade in or fade out curve for a clip.
#[allow(clippy::too_many_arguments)]
fn draw_fade_curve(
    painter: &egui::Painter,
    clip: &AudioClip,
    fade_in: bool,
    lane: egui::Rect,
    view: TimelineView,
    source_sr: f64,
    clip_start: f64,
    clip_rect: egui::Rect,
) {
    let fade = if fade_in {
        &clip.fade_in
    } else {
        &clip.fade_out
    };
    let Some(fade) = fade else { return };
    if fade.length == 0 {
        return;
    }

    let fade_secs = fade.length as f64 / source_sr;
    let start_secs = if fade_in {
        clip_start
    } else {
        clip_start + clip.length as f64 / source_sr - fade_secs
    };

    let start_x = view.x(lane, start_secs);
    let end_x = view.x(lane, start_secs + fade_secs);
    if end_x < lane.left() - 2.0 || start_x > lane.right() + 2.0 {
        return;
    }

    let steps = ((end_x - start_x).abs().clamp(2.0, 256.0)) as usize;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let gain = if fade_in {
            fade.curve.apply(t)
        } else {
            1.0 - fade.curve.apply(t)
        };
        let x = start_x + (end_x - start_x) * t;
        let y = clip_rect.top() + (1.0 - gain) * clip_rect.height();
        points.push(egui::pos2(x, y));
    }

    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(110)),
    ));
}

/// Combined fade gain at a sample offset within the clip.
fn fade_gain_at(clip: &AudioClip, offset_samples: f64) -> f32 {
    let offset = offset_samples.max(0.0) as u64;
    let mut gain = 1.0f32;

    if let Some(fade) = &clip.fade_in {
        if offset < fade.length {
            gain *= fade.gain_at(offset);
        }
    }

    if let Some(fade) = &clip.fade_out {
        if fade.length > 0 {
            let start = clip.length.saturating_sub(fade.length);
            if offset >= start {
                let t = ((offset - start) as f32 / fade.length as f32).clamp(0.0, 1.0);
                gain *= 1.0 - fade.curve.apply(t);
            }
        }
    }

    gain.clamp(0.0, 4.0)
}

/// Read a mono mix of `frames` frames starting at `start_frame`.
fn read_mono_window(
    path: &Path,
    start_frame: u64,
    frames: usize,
    _channels: u16,
    _sample_rate: u32,
) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;

    if start_frame > 0 {
        let seek_to = start_frame.min(u32::MAX as u64) as u32;
        reader.seek(seek_to)?;
    }

    let mut out = Vec::with_capacity(frames);

    match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut samples = reader.samples::<f32>();
            'frames: for _ in 0..frames {
                let mut sum = 0.0f32;
                let mut count = 0usize;
                for _ in 0..channels {
                    match samples.next() {
                        Some(Ok(v)) => {
                            sum += v;
                            count += 1;
                        }
                        _ => break 'frames,
                    }
                }
                out.push(sum / count.max(1) as f32);
            }
        }
        hound::SampleFormat::Int => {
            let scale = pcm_scale(spec.bits_per_sample);
            let mut samples = reader.samples::<i32>();
            'frames: for _ in 0..frames {
                let mut sum = 0.0f32;
                let mut count = 0usize;
                for _ in 0..channels {
                    match samples.next() {
                        Some(Ok(v)) => {
                            sum += v as f32 * scale;
                            count += 1;
                        }
                        _ => break 'frames,
                    }
                }
                out.push(sum / count.max(1) as f32);
            }
        }
    }

    Ok(out)
}

/// Pick a "nice" ruler interval (seconds) that is at least ~70 px wide.
fn nice_interval(pps: f32) -> f64 {
    const CANDIDATES: [f64; 21] = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0,
        30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 3600.0,
    ];
    for interval in CANDIDATES {
        if interval * pps as f64 >= 70.0 {
            return interval;
        }
    }
    *CANDIDATES.last().unwrap()
}

fn format_ruler_time(secs: f64, interval: f64) -> String {
    let minutes = (secs / 60.0).floor().max(0.0) as u64;
    let rem = secs - minutes as f64 * 60.0;
    if interval >= 1.0 {
        format!("{}:{:02}", minutes, rem.round() as u64 % 60)
    } else if interval >= 0.1 {
        format!("{}:{:04.1}", minutes, rem)
    } else if interval >= 0.01 {
        format!("{}:{:05.2}", minutes, rem)
    } else {
        format!("{}:{:06.3}", minutes, rem)
    }
}

fn format_clock(secs: f64) -> String {
    let secs = secs.max(0.0);
    let minutes = (secs / 60.0).floor() as u64;
    let rem = secs - minutes as f64 * 60.0;
    format!("{:02}:{:06.3}", minutes, rem)
}

/// Strip generated processing suffixes such as `_reverse_<uuid>` so repeated
/// renders keep a stable base name instead of growing on every operation.
fn processed_file_stem(path: &Path) -> String {
    const MARKERS: [&str; 4] = ["_reverse_", "_process_", "_pitch_", "_stretch_"];

    let mut stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("clip")
        .to_string();

    loop {
        let mut stripped = false;
        for marker in MARKERS {
            if let Some(index) = stem.rfind(marker) {
                let suffix = &stem[index + marker.len()..];
                if uuid::Uuid::parse_str(suffix).is_ok() {
                    stem.truncate(index);
                    stripped = true;
                    break;
                }
            }
        }
        if !stripped {
            break;
        }
    }

    if stem.is_empty() {
        "clip".to_string()
    } else {
        stem
    }
}

/// Reverse interleaved audio frame-wise, preserving channel order inside each
/// frame. A trailing incomplete frame is dropped.
fn reverse_interleaved(samples: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    let mut reversed = Vec::with_capacity(frames * channels);
    for frame in samples[..frames * channels].chunks_exact(channels).rev() {
        reversed.extend_from_slice(frame);
    }
    reversed
}

/// Convert integer PCM samples at the given bit depth to [-1, 1] floats.
fn pcm_scale(bits: u16) -> f32 {
    match bits {
        8 => 1.0 / 128.0,
        16 => 1.0 / 32_768.0,
        24 => 1.0 / 8_388_608.0,
        32 => 1.0 / 2_147_483_648.0,
        _ => 1.0 / 32_768.0,
    }
}

/// Map a WAV format to the project's bit-depth enum.
fn bit_depth_from_spec(spec: hound::WavSpec) -> BitDepth {
    match spec.bits_per_sample {
        16 => BitDepth::Int16,
        24 => BitDepth::Int24,
        32 if spec.sample_format == hound::SampleFormat::Float => BitDepth::Float32,
        32 => BitDepth::Int32,
        _ => BitDepth::Float32,
    }
}

/// Bits per sample for a project bit depth.
fn bit_depth_bits(depth: BitDepth) -> u16 {
    match depth {
        BitDepth::Int16 => 16,
        BitDepth::Int24 => 24,
        BitDepth::Int32 | BitDepth::Float32 => 32,
    }
}

/// Read only a WAV header: duration in frames and format.
fn wav_file_info(path: &Path) -> Result<(u64, hound::WavSpec)> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    Ok((reader.duration() as u64, spec))
}

/// Write a frame range from a WAV to a new 32-bit float WAV, converting
/// integer PCM as needed. Only the requested range is read from the source.
fn write_wav_range(
    source_path: &Path,
    first_frame: u64,
    frames: u64,
    dest: &Path,
) -> Result<()> {
    write_wav_range_with_envelope(source_path, first_frame, frames, dest, |_| 1.0)
}

/// Like [`write_wav_range`], but applies a per-frame gain envelope while
/// copying. The envelope index is relative to the start of the range.
fn write_wav_range_with_envelope<F>(
    source_path: &Path,
    first_frame: u64,
    frames: u64,
    dest: &Path,
    mut gain_at: F,
) -> Result<()>
where
    F: FnMut(u64) -> f32,
{
    let mut reader = hound::WavReader::open(source_path)?;
    let spec = reader.spec();
    let total_frames = reader.duration() as u64;
    let first = first_frame.min(total_frames);
    let frames = frames.min(total_frames - first);
    if frames == 0 {
        anyhow::bail!("no audio in the selected range");
    }
    reader.seek(first as u32)?;

    let channels = spec.channels.max(1);
    let out_spec = hound::WavSpec {
        channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dest, out_spec)?;

    match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut samples = reader.samples::<f32>();
            for frame in 0..frames {
                let gain = gain_at(frame);
                for _ in 0..channels {
                    let sample = samples.next().transpose()?.unwrap_or(0.0);
                    writer.write_sample(sample * gain)?;
                }
            }
        }
        hound::SampleFormat::Int => {
            let scale = pcm_scale(spec.bits_per_sample);
            let mut samples = reader.samples::<i32>();
            for frame in 0..frames {
                let gain = gain_at(frame);
                for _ in 0..channels {
                    let sample =
                        samples.next().transpose()?.unwrap_or(0) as f32 * scale;
                    writer.write_sample(sample * gain)?;
                }
            }
        }
    }

    writer.finalize()?;
    Ok(())
}

/// Write interleaved samples as a new 32-bit float WAV.
fn write_wav_f32(
    path: &Path,
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Result<()> {
    let spec = hound::WavSpec {
        channels: channels.max(1),
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for sample in samples {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Run Rubber Band in fine (R3) mode to pitch-shift and/or time-stretch a WAV.
fn render_pitch_stretch(
    input: &Path,
    output: &Path,
    semitones: f32,
    stretch: f32,
    preserve_formants: bool,
    mode: TimeStretchMode,
) -> Result<()> {
    let pitch_active = semitones.abs() >= 0.05;
    let stretch_active = (stretch - 1.0).abs() >= 0.005;
    if !pitch_active && !stretch_active {
        anyhow::bail!("no pitch or time change requested");
    }

    let mut command = std::process::Command::new("rubberband");
    command.arg("--quiet").arg("--fine");
    if mode == TimeStretchMode::Musical {
        command.arg("--centre-focus");
    }
    if pitch_active {
        command.arg("--pitch").arg(format!("{:.4}", semitones));
        if preserve_formants {
            command.arg("--formant");
        }
    }
    if stretch_active {
        command.arg("--time").arg(format!("{:.6}", stretch));
    }
    command.arg(input).arg(output);

    let result = command
        .output()
        .map_err(|e| anyhow::anyhow!("could not run rubberband: {}", e))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let message = stderr.trim();
        if message.is_empty() {
            anyhow::bail!("rubberband exited with {}", result.status);
        }
        anyhow::bail!("rubberband: {}", message);
    }
    Ok(())
}

/// Tape-style varispeed: resample the clip so length and pitch change together.
/// `stretch` above 1 makes the clip longer and lower; below 1 makes it shorter
/// and higher.
fn tape_stretch_interleaved(samples: &[f32], channels: u16, stretch: f32) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let frames = samples.len() / channels;
    let stretch = stretch.clamp(0.25, 4.0) as f64;
    if frames == 0 {
        return Vec::new();
    }
    let out_frames = ((frames as f64 * stretch).round() as usize).max(1);
    let mut out = vec![0.0f32; out_frames * channels];

    for frame in 0..out_frames {
        let src_pos = frame as f64 / stretch;
        let i0 = (src_pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let frac = (src_pos - i0 as f64) as f32;
        for channel in 0..channels {
            let a = samples[i0 * channels + channel];
            let b = samples[i1 * channels + channel];
            out[frame * channels + channel] = a + (b - a) * frac;
        }
    }

    out
}

/// Reduce bit depth and hold samples for a crunchy, digital character.
fn bit_crush(samples: &mut [f32], channels: u16, bits: u32, downsample: u32) {
    let channel_count = channels.max(1) as usize;
    if samples.is_empty() {
        return;
    }
    let bits = bits.clamp(1, 24);
    let steps = 2f32.powi(bits as i32 - 1);
    let factor = downsample.max(1) as usize;
    let frames = samples.len() / channel_count;
    let mut held = vec![0.0f32; channel_count];

    for frame in 0..frames {
        let base = frame * channel_count;
        if frame % factor == 0 {
            held.copy_from_slice(&samples[base..base + channel_count]);
        }
        for channel in 0..channel_count {
            let quantized =
                (held[channel].clamp(-1.0, 1.0) * steps).round() / steps;
            samples[base + channel] = quantized;
        }
    }
}

/// Soft-clip the signal with adjustable drive and dry/wet mix.
fn saturate(samples: &mut [f32], drive_db: f32, mix: f32) {
    let mix = mix.clamp(0.0, 1.0);
    if mix <= 0.0 {
        return;
    }
    let drive = 10f32.powf(drive_db.clamp(0.0, 48.0) / 20.0);
    let norm = if drive > 1e-3 {
        drive.tanh().recip()
    } else {
        1.0
    };

    for sample in samples {
        let dry = *sample;
        let wet = (dry * drive).tanh() * norm;
        *sample = dry * (1.0 - mix) + wet * mix;
    }
}

/// Apply a first-order low-pass or high-pass filter in place.
fn filter_audio(
    samples: &mut [f32],
    channels: u16,
    sample_rate: u32,
    cutoff: f32,
    kind: FilterKind,
) {
    let channel_count = channels.max(1) as usize;
    if samples.is_empty() {
        return;
    }
    let sample_rate = sample_rate.max(1) as f32;
    let max_cutoff = (sample_rate * 0.475).max(1.0);
    let cutoff = cutoff.clamp(1.0, max_cutoff);
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / sample_rate).exp();
    let frames = samples.len() / channel_count;
    let mut state = vec![0.0f32; channel_count];

    for frame in 0..frames {
        let base = frame * channel_count;
        for channel in 0..channel_count {
            let input = samples[base + channel];
            state[channel] += alpha * (input - state[channel]);
            samples[base + channel] = match kind {
                FilterKind::LowPass => state[channel],
                FilterKind::HighPass => input - state[channel],
            };
        }
    }
}

/// Modulate clip volume with a sine LFO.
fn tremolo(samples: &mut [f32], channels: u16, sample_rate: u32, rate: f32, depth: f32) {
    let channel_count = channels.max(1) as usize;
    if samples.is_empty() {
        return;
    }
    let depth = depth.clamp(0.0, 1.0);
    if depth <= 0.0 {
        return;
    }
    let sample_rate = sample_rate.max(1) as f32;
    let rate = rate.clamp(0.0, sample_rate * 0.5);
    let frames = samples.len() / channel_count;

    for frame in 0..frames {
        let t = frame as f32 / sample_rate;
        let modulation =
            1.0 - depth * 0.5 * (1.0 - (std::f32::consts::TAU * rate * t).cos());
        let base = frame * channel_count;
        for channel in 0..channel_count {
            samples[base + channel] *= modulation;
        }
    }
}

/// Peak absolute sample of a WAV frame range, without loading the whole file.
fn wav_peak(path: &Path, first_frame: u64, frames: u64) -> Result<f32> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as u64;
    let total_frames = reader.duration() as u64;
    let first = first_frame.min(total_frames);
    let frames = frames.min(total_frames - first);
    if frames == 0 {
        return Ok(0.0);
    }
    reader.seek(first.min(u32::MAX as u64) as u32)?;

    let scale = pcm_scale(spec.bits_per_sample);
    let sample_count = (frames * channels) as usize;
    let mut peak = 0.0f32;

    if spec.sample_format == hound::SampleFormat::Float {
        for sample in reader.samples::<f32>().take(sample_count) {
            if let Ok(sample) = sample {
                peak = peak.max(sample.abs());
            }
        }
    } else {
        for sample in reader.samples::<i32>().take(sample_count) {
            if let Ok(sample) = sample {
                peak = peak.max((sample as f32 * scale).abs());
            }
        }
    }

    Ok(peak)
}

/// Read a WAV frame range into interleaved f32 samples.
fn load_wav_range_interleaved(
    path: &Path,
    first_frame: u64,
    frames: u64,
) -> Result<(Vec<f32>, u16, u32)> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let total_frames = reader.duration() as u64;
    let first = first_frame.min(total_frames);
    let frames = frames.min(total_frames - first);
    if frames == 0 {
        anyhow::bail!("no audio in the selected range");
    }
    reader.seek(first as u32)?;

    let channels = spec.channels.max(1);
    let sample_count = (frames * channels as u64) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .take(sample_count)
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            let scale = pcm_scale(spec.bits_per_sample);
            reader
                .samples::<i32>()
                .take(sample_count)
                .filter_map(std::result::Result::ok)
                .map(|sample| sample as f32 * scale)
                .collect()
        }
    };

    Ok((samples, channels, spec.sample_rate))
}

/// Read an entire WAV file into interleaved f32 samples.
fn load_wav_interleaved(path: &Path) -> Result<(Vec<f32>, u16, u32)> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            let scale = pcm_scale(spec.bits_per_sample);
            reader
                .samples::<i32>()
                .filter_map(std::result::Result::ok)
                .map(|s| s as f32 * scale)
                .collect()
        }
    };

    Ok((samples, spec.channels, spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::{
        audible_track_gain, bit_crush, bpm_from_tap_intervals, clamp_group_delta, clip_gain_at,
        clip_span_samples, clips_in_marquee, contiguous_clip_span, filter_audio,
        knob_from_normalized, knob_to_normalized, load_wav_interleaved,
        load_wav_range_interleaved, loop_range_frames, measure_integrated_loudness,
        merge_continuous_edit, metronome_config, midi_clip_note_spans, midi_clip_notes,
        cc_pan_gains, midi_cc_value_at, pitch_in_scale,
        quantize_recorded_midi_clip, filter_midi_events,
        fold_midi_clip_to_loop,
        slider_wheel_value, slider_wheel_value_with_step,
        slider_wheel_log_value,
        midi_clip_content_notes, midi_clips_in_marquee, note_name, processed_file_stem,
        punch_waiting, render_pitch_stretch, reverse_interleaved, saturate, take_lane_layout,
        tape_stretch_interleaved, timeline_snap_ticks, tremolo, wav_file_info, wav_peak,
        write_piano_roll_notes, write_wav_f32, write_wav_range, write_wav_range_with_envelope,
        fit_piano_roll_view, piano_roll_new_note_length, midi_trim_end, midi_trim_start,
        fit_piano_roll_selection,
        split_midi_clip_at_tick, quantize_midi_notes, scale_midi_velocities,
        compress_midi_velocities, expand_midi_velocities, curve_midi_velocities,
        transpose_midi_notes, scale_conform_midi_notes, velocity_color, notes_in_marquee,
        midi_clip_cc_points, write_midi_cc_points, humanize_midi_notes, legato_midi_notes,
        set_midi_note_length, reverse_midi_notes, mirror_midi_notes, randomize_midi_notes,
        join_midi_notes, join_note_with_next, split_midi_notes_at_tick, thin_midi_notes,
        ghost_notes_for_clip,
        MidiCcPoint, project_from_template, ClipShape, ClipSourceState,
        EditAction, FilterKind, MonitorMode, PianoRollNote, PianoRollSnap, PianoRollWindow,
        SnapMode, Template, TrackShape,
    };
    use daw_core::audio_pool::AudioFileRef;
    use daw_core::clip::{
        AudioClip, ClipId, Fade, FadeCurve, MidiClip, MidiEvent, TimeStretchMode, TimelinePos,
    };
    use daw_core::project::BitDepth;
    use daw_core::undo::History;
    use daw_core::Project;

    fn shape(gain: f32) -> ClipShape {
        ClipShape {
            source_offset: 0,
            position: TimelinePos::default(),
            length: 100,
            gain,
            fade_in: None,
            fade_out: None,
            color: None,
            muted: false,
        }
    }

    #[test]
    fn edit_action_descriptions_name_the_change() {
        let clip_id = ClipId::default();

        let gain = EditAction::Shape {
            clip_id,
            from: shape(1.0),
            to: shape(2.0),
        };
        assert_eq!(gain.description(), "Clip gain");

        let mut fade_to = shape(1.0);
        fade_to.fade_in = Some(Fade::new(10, FadeCurve::Linear));
        let fade = EditAction::Shape {
            clip_id,
            from: shape(1.0),
            to: fade_to,
        };
        assert_eq!(fade.description(), "Fade in");

        let mut trim_to = shape(1.0);
        trim_to.length = 50;
        let trim = EditAction::Shape {
            clip_id,
            from: shape(1.0),
            to: trim_to,
        };
        assert_eq!(trim.description(), "Trim clip");

        let group = EditAction::Group {
            actions: Vec::new(),
            label: "Split clip".to_string(),
        };
        assert_eq!(group.description(), "Split clip");

        let mut muted_to = shape(1.0);
        muted_to.muted = true;
        let muted = EditAction::Shape {
            clip_id,
            from: shape(1.0),
            to: muted_to,
        };
        assert_eq!(muted.description(), "Mute take");

        let moved = EditAction::Move {
            clip_id,
            from_track: daw_core::track::TrackId::default(),
            from_pos: TimelinePos::default(),
            to_track: daw_core::track::TrackId::default(),
            to_pos: TimelinePos::default(),
        };
        assert_eq!(moved.description(), "Move clip");
    }

    #[test]
    fn track_insert_descriptions_name_the_change() {
        let track_id = daw_core::track::TrackId::default();
        let from = Vec::new();
        let to = vec![daw_core::track::InsertSlot::with_effect(
            daw_core::track::TrackEffect::default_saturation(),
        )];
        let add = EditAction::TrackInserts {
            track_id,
            from: from.clone(),
            to: to.clone(),
        };
        assert_eq!(add.description(), "Add track effect");

        let remove = EditAction::TrackInserts {
            track_id,
            from: to.clone(),
            to: from,
        };
        assert_eq!(remove.description(), "Remove track effect");

        let mut changed = to.clone();
        changed[0].mix = 0.5;
        let edit = EditAction::TrackInserts {
            track_id,
            from: to,
            to: changed,
        };
        assert_eq!(edit.description(), "Track effects");
    }

    #[test]
    fn track_state_descriptions_name_the_toggle() {
        let track_id = daw_core::track::TrackId::default();
        let base = TrackShape {
            mute: false,
            solo: false,
            record_arm: false,
            volume: 1.0,
            pan: daw_core::track::PanLaw::center(),
            monitor_mode: MonitorMode::Off,
        };

        let mut muted = base;
        muted.mute = true;
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: muted,
        };
        assert_eq!(action.description(), "Mute track");

        let mut soloed = base;
        soloed.solo = true;
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: soloed,
        };
        assert_eq!(action.description(), "Solo track");

        let mut armed = base;
        armed.record_arm = true;
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: armed,
        };
        assert_eq!(action.description(), "Arm track");

        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: base,
        };
        assert_eq!(action.description(), "Track change");

        let mut louder = base;
        louder.volume = 0.5;
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: louder,
        };
        assert_eq!(action.description(), "Track volume");

        let mut panned = base;
        panned.pan = daw_core::track::PanLaw::from_linear(-0.5);
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: panned,
        };
        assert_eq!(action.description(), "Track pan");

        let mut monitoring = base;
        monitoring.monitor_mode = MonitorMode::Auto;
        let action = EditAction::TrackState {
            track_id,
            from: base,
            to: monitoring,
        };
        assert_eq!(action.description(), "Monitor auto");
    }

    #[test]
    fn bpm_from_tap_intervals_averages_and_clamps() {
        assert_eq!(bpm_from_tap_intervals(&[]), None);

        let bpm = bpm_from_tap_intervals(&[0.5]).unwrap();
        assert!((bpm - 120.0).abs() < 1e-3);

        let bpm = bpm_from_tap_intervals(&[0.5, 0.5, 0.5]).unwrap();
        assert!((bpm - 120.0).abs() < 1e-3);

        assert_eq!(bpm_from_tap_intervals(&[0.01]), Some(300.0));
        assert_eq!(bpm_from_tap_intervals(&[10.0]), Some(20.0));
    }

    #[test]
    fn metronome_config_follows_tempo_and_time_signature() {
        assert!(metronome_config(false, 120.0, 4, 4, 48_000).is_none());

        let config = metronome_config(true, 120.0, 4, 4, 48_000).unwrap();
        assert!((config.frames_per_beat - 24_000.0).abs() < 1e-6);
        assert_eq!(config.beats_per_bar, 4);
        assert!(config.gain > 0.0);

        let config = metronome_config(true, 120.0, 6, 8, 48_000).unwrap();
        assert!((config.frames_per_beat - 12_000.0).abs() < 1e-6);
        assert_eq!(config.beats_per_bar, 6);
    }

    #[test]
    fn punch_waiting_waits_only_before_the_in_point() {
        assert!(punch_waiting(true, 2.0, 4.0, 1.0));
        // At the in point the take starts.
        assert!(!punch_waiting(true, 2.0, 4.0, 2.0));
        assert!(!punch_waiting(true, 2.0, 4.0, 3.0));
        assert!(!punch_waiting(false, 2.0, 4.0, 1.0));
        // An empty range behaves like punch disabled.
        assert!(!punch_waiting(true, 4.0, 2.0, 1.0));
    }

    #[test]
    fn loop_range_frames_converts_seconds_and_disables_empty_ranges() {
        assert_eq!(loop_range_frames(false, 0.0, 1.0, 10), None);
        assert_eq!(loop_range_frames(true, 0.0, 1.0, 10), Some((0, 10)));
        assert_eq!(loop_range_frames(true, 0.5, 1.0, 10), Some((5, 10)));
        assert_eq!(loop_range_frames(true, 1.0, 1.0, 10), None);
        assert_eq!(loop_range_frames(true, 1.0, 0.5, 10), None);
        // The end is rounded up so a fractional frame is not lost.
        assert_eq!(loop_range_frames(true, 0.0, 0.15, 10), Some((0, 2)));
    }

    #[test]
    fn track_mute_solo_and_volume_feed_the_mixer_gain() {
        let mut track = daw_core::track::Track::new_audio("test");
        assert_eq!(audible_track_gain(&track, false), Some(1.0));

        track.mute = true;
        assert_eq!(audible_track_gain(&track, false), None);

        track.mute = false;
        track.solo = true;
        assert_eq!(audible_track_gain(&track, true), Some(1.0));

        track.solo = false;
        assert_eq!(audible_track_gain(&track, true), None);

        track.volume = 0.5;
        assert_eq!(audible_track_gain(&track, false), Some(0.5));

        track.volume = 10.0;
        assert_eq!(audible_track_gain(&track, false), Some(4.0));
    }

    #[test]
    fn continuous_shape_edits_merge_into_one_entry() {
        let clip_id = ClipId::default();
        let mut history: History<EditAction> = History::new(10);

        let first = EditAction::Shape {
            clip_id,
            from: shape(1.0),
            to: shape(1.5),
        };
        let second = EditAction::Shape {
            clip_id,
            from: shape(1.5),
            to: shape(2.0),
        };

        history.push(first, "Clip gain".to_string());
        assert!(merge_continuous_edit(&mut history, &second));
        assert_eq!(history.past_len(), 1);

        let EditAction::Shape { to, .. } = &history.past()[0].action else {
            panic!("expected a shape edit");
        };
        assert_eq!(to.gain, 2.0);
        assert_eq!(history.past()[0].description, "Clip gain");
    }

    #[test]
    fn continuous_track_state_edits_merge_into_one_entry() {
        let track_id = daw_core::track::TrackId::default();
        let base = TrackShape {
            mute: false,
            solo: false,
            record_arm: false,
            volume: 1.0,
            pan: daw_core::track::PanLaw::center(),
            monitor_mode: MonitorMode::Off,
        };
        let mut history: History<EditAction> = History::new(10);

        let first = EditAction::TrackState {
            track_id,
            from: base,
            to: TrackShape {
                volume: 0.9,
                ..base
            },
        };
        let second = EditAction::TrackState {
            track_id,
            from: TrackShape {
                volume: 0.9,
                ..base
            },
            to: TrackShape {
                volume: 0.7,
                ..base
            },
        };

        history.push(first, "Track volume".to_string());
        assert!(merge_continuous_edit(&mut history, &second));
        assert_eq!(history.past_len(), 1);

        let EditAction::TrackState { to, .. } = &history.past()[0].action else {
            panic!("expected a track state edit");
        };
        assert_eq!(to.volume, 0.7);
        assert_eq!(history.past()[0].description, "Track volume");
    }

    #[test]
    fn slider_wheel_moves_five_percent_per_notch() {
        assert_eq!(slider_wheel_value(50.0, 0.0, 100.0, 1.0), 55.0);
        assert_eq!(slider_wheel_value(50.0, 0.0, 100.0, -1.0), 45.0);
        assert_eq!(slider_wheel_value(2.0, 0.0, 10.0, 1.0), 2.5);
        assert_eq!(slider_wheel_value(99.0, 0.0, 100.0, 2.0), 100.0);
        assert_eq!(slider_wheel_value(1.0, 0.0, 100.0, -2.0), 0.0);
        assert_eq!(
            slider_wheel_value_with_step(1.0, 0.0, 1.5, 1.0, 0.05),
            1.05
        );
        assert_eq!(
            slider_wheel_value_with_step(1.0, 0.0, 1.5, -1.0, 0.05),
            0.95
        );
        let log_up = slider_wheel_log_value(100.0, 20.0, 20_000.0, 1.0);
        let log_down = slider_wheel_log_value(100.0, 20.0, 20_000.0, -1.0);
        assert!((log_up - 141.25).abs() < 0.1);
        assert!((log_down - 70.79).abs() < 0.1);
    }

    #[test]
    fn midi_channel_filter_keeps_only_selected_channel() {
        let event = |channel: u8| daw_engine::MidiInputEvent {
            timestamp_micros: 0,
            received_at: std::time::Instant::now(),
            channel,
            note: 60,
            velocity: 100,
            on: true,
        };
        let filtered =
            filter_midi_events(vec![event(0), event(1), event(2)], Some(1));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].channel, 1);

        let all = filter_midi_events(vec![event(0), event(2)], None);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn midi_cc_value_at_uses_latest_point() {
        let points = vec![
            MidiCcPoint {
                tick: 0,
                value: 10,
                channel: 0,
            },
            MidiCcPoint {
                tick: 240,
                value: 90,
                channel: 0,
            },
        ];
        assert_eq!(midi_cc_value_at(&points, 0, 64), 10);
        assert_eq!(midi_cc_value_at(&points, 239, 64), 10);
        assert_eq!(midi_cc_value_at(&points, 240, 64), 90);
        assert_eq!(midi_cc_value_at(&points, 500, 64), 90);
        assert_eq!(midi_cc_value_at(&[], 500, 64), 64);
    }

    #[test]
    fn glue_tool_joins_only_nearby_same_pitch_notes() {
        let note = |start, end, pitch| PianoRollNote {
            start,
            end,
            pitch,
            velocity: 100,
            channel: 0,
        };
        let mut notes = vec![
            note(0, 240, 60),
            note(300, 480, 60),
            note(960, 1200, 60),
            note(300, 480, 64),
        ];
        assert!(join_note_with_next(&mut notes, 0, 120));
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].start, notes[0].end), (0, 480));
        assert!(!join_note_with_next(&mut notes, 0, 120));
        assert_eq!(notes.len(), 3);
    }

    #[test]
    fn loop_fold_wraps_repeated_passes_into_the_loop() {
        let mut clip = MidiClip::new(TimelinePos::new(0, 0), 3840);
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));
        clip.add_event(MidiEvent::note_off(240, 0, 60, 0));
        clip.add_event(MidiEvent::note_on(1920, 0, 64, 100));
        clip.add_event(MidiEvent::note_off(2160, 0, 64, 0));
        let folded = fold_midi_clip_to_loop(&clip, 0, 1920, 0);
        let notes = midi_clip_content_notes(&folded);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].start, notes[0].end, notes[0].pitch), (0, 240, 60));
        assert_eq!((notes[1].start, notes[1].end, notes[1].pitch), (0, 240, 64));
        assert_eq!(folded.length, 1920);
    }

    #[test]
    fn loop_fold_truncates_notes_at_the_loop_boundary() {
        let mut clip = MidiClip::new(TimelinePos::new(0, 0), 1920);
        clip.add_event(MidiEvent::note_on(1800, 0, 67, 100));
        clip.add_event(MidiEvent::note_off(2100, 0, 67, 0));
        let folded = fold_midi_clip_to_loop(&clip, 0, 1920, 0);
        let notes = midi_clip_content_notes(&folded);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].start, notes[0].end), (1800, 1920));
    }

    #[test]
    fn input_quantize_aligns_recorded_note_starts() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.add_event(MidiEvent::note_on(100, 0, 60, 100));
        clip.add_event(MidiEvent::note_off(340, 0, 60, 0));
        quantize_recorded_midi_clip(&mut clip, 240);
        let notes = midi_clip_content_notes(&clip);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].start, notes[0].end), (0, 240));
    }

    #[test]
    fn pitch_in_scale_matches_major_and_minor() {
        let major = [0, 2, 4, 5, 7, 9, 11];
        assert!(pitch_in_scale(60, 0, &major));
        assert!(!pitch_in_scale(61, 0, &major));
        assert!(pitch_in_scale(72, 0, &major));

        let minor = [0, 2, 3, 5, 7, 8, 10];
        assert!(pitch_in_scale(60, 0, &minor));
        assert!(pitch_in_scale(63, 0, &minor));
        assert!(!pitch_in_scale(64, 0, &minor));
    }

    #[test]
    fn cc10_pan_maps_left_center_and_right() {
        let (left, right) = cc_pan_gains(0);
        assert!(left > 0.0);
        assert!(right.abs() < 1e-6);

        let (left, right) = cc_pan_gains(64);
        assert!((left - 1.0).abs() < 0.02);
        assert!((right - 1.0).abs() < 0.02);

        let (left, right) = cc_pan_gains(127);
        assert!(left.abs() < 1e-6);
        assert!(right > 0.0);
    }

    #[test]
    fn processed_stem_strips_generated_suffixes() {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();

        let once = std::path::PathBuf::from(format!("take_reverse_{}.wav", first));
        assert_eq!(processed_file_stem(&once), "take");

        // Folders created by older builds may already contain stacked suffixes.
        let twice = std::path::PathBuf::from(format!(
            "take_reverse_{}_reverse_{}.wav",
            second, first
        ));
        assert_eq!(processed_file_stem(&twice), "take");
    }

    #[test]
    fn reverse_preserves_channel_order_within_frames() {
        let input = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0];
        let reversed = reverse_interleaved(&input, 2);
        assert_eq!(reversed, vec![3.0, 30.0, 2.0, 20.0, 1.0, 10.0]);
    }

    #[test]
    fn reverse_drops_incomplete_trailing_frame() {
        let input = [1.0, 10.0, 2.0, 20.0, 9.0];
        let reversed = reverse_interleaved(&input, 2);
        assert_eq!(reversed, vec![2.0, 20.0, 1.0, 10.0]);
    }

    #[test]
    fn source_state_restores_length_fades_and_gain() {
        let source = AudioFileRef::new(
            std::path::PathBuf::from("source.wav"),
            1_000,
            48_000,
            2,
            BitDepth::Float32,
        );
        let mut clip = AudioClip::new(
            source,
            TimelinePos::from_sample(0, 48_000.0, 120.0),
            1_000,
        );
        clip.source_offset = 25;
        clip.fade_in = Some(Fade::new(120, FadeCurve::Linear));
        clip.gain = 0.5;

        let before = ClipSourceState::of(&clip);
        clip.source_offset = 0;
        clip.length = 500;
        clip.fade_in = Some(Fade::new(50, FadeCurve::Exponential));
        clip.gain = 1.25;
        let after = ClipSourceState::of(&clip);

        after.apply(&mut clip);
        assert_eq!(clip.source_offset, 0);
        assert_eq!(clip.length, 500);
        assert_eq!(clip.fade_in.unwrap().length, 50);
        assert!((clip.gain - 1.25).abs() < 1e-6);

        before.apply(&mut clip);
        assert_eq!(clip.source_offset, 25);
        assert_eq!(clip.length, 1_000);
        assert_eq!(clip.fade_in.unwrap().length, 120);
        assert!((clip.gain - 0.5).abs() < 1e-6);
    }

    #[test]
    fn write_wav_range_extracts_selected_frames() {
        let input = std::env::temp_dir().join(format!(
            "zoft_range_in_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let output = std::env::temp_dir().join(format!(
            "zoft_range_out_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&input, spec).unwrap();
        for frame in [[0.25f32, -0.25], [0.5, -0.5], [-0.75, 0.75]] {
            writer
                .write_sample((frame[0] * 32_767.0) as i16)
                .unwrap();
            writer
                .write_sample((frame[1] * 32_767.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();

        write_wav_range(&input, 1, 2, &output).unwrap();
        let (frames, out_spec) = wav_file_info(&output).unwrap();
        assert_eq!(frames, 2);
        assert_eq!(out_spec.channels, 2);
        assert_eq!(out_spec.sample_rate, 48_000);
        assert_eq!(out_spec.bits_per_sample, 32);
        assert_eq!(out_spec.sample_format, hound::SampleFormat::Float);

        let (samples, _, _) = load_wav_interleaved(&output).unwrap();
        assert_eq!(samples.len(), 4);
        assert!((samples[0] - 0.5).abs() < 1e-3);
        assert!((samples[1] + 0.5).abs() < 1e-3);
        assert!((samples[2] + 0.75).abs() < 1e-3);
        assert!((samples[3] - 0.75).abs() < 1e-3);

        std::fs::remove_file(input).ok();
        std::fs::remove_file(output).ok();
    }

    #[test]
    fn clip_gain_applies_fades_at_source_offsets() {
        let fade_in = Some(Fade::new(10, FadeCurve::Linear));
        let fade_out = Some(Fade::new(20, FadeCurve::Linear));

        // First frame of a fade-in is silent.
        assert!(clip_gain_at(0, 100, 1.0, fade_in, fade_out).abs() < 1e-6);
        // Halfway through the 10-frame fade-in is half gain.
        assert!((clip_gain_at(5, 100, 1.0, fade_in, fade_out) - 0.5).abs() < 1e-6);
        // Middle of the clip is unaffected.
        assert!((clip_gain_at(50, 100, 1.0, fade_in, fade_out) - 1.0).abs() < 1e-6);
        // The 20-frame fade-out starts at frame 80; frame 90 is half gain.
        assert!((clip_gain_at(90, 100, 1.0, fade_in, fade_out) - 0.5).abs() < 1e-6);
        // Clip gain is applied and limited to the mixer's 4x ceiling.
        assert!((clip_gain_at(50, 100, 0.5, None, None) - 0.5).abs() < 1e-6);
        assert!((clip_gain_at(50, 100, 10.0, None, None) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn knob_normalization_round_trips_linear_and_logarithmic_values() {
        for value in [-12.0f32, 0.0, 12.0] {
            let norm = knob_to_normalized(value, -12.0, 12.0, false);
            assert!((norm - (value + 12.0) / 24.0).abs() < 1e-6);
            let round_trip = knob_from_normalized(norm, -12.0, 12.0, false);
            assert!((round_trip - value).abs() < 1e-6);
        }

        for value in [20.0f32, 1_000.0, 20_000.0] {
            let norm = knob_to_normalized(value, 20.0, 20_000.0, true);
            let round_trip = knob_from_normalized(norm, 20.0, 20_000.0, true);
            assert!((round_trip - value).abs() < 0.1);
        }
    }

    #[test]
    fn contiguous_spans_expand_across_touching_and_overlapping_clips() {
        let mut project = Project::new("test");
        let source = AudioFileRef::new(
            std::path::PathBuf::from("source.wav"),
            100,
            48_000,
            1,
            BitDepth::Float32,
        );
        let template = AudioClip::new(source, TimelinePos::default(), 1);
        let a = project.audio_clips.insert(template.clone());
        let b = project.audio_clips.insert(template.clone());
        let c = project.audio_clips.insert(template.clone());
        let d = project.audio_clips.insert(template.clone());
        let spans = [(a, 0, 10), (b, 10, 20), (c, 18, 30), (d, 40, 50)];

        let (ids, start, end) = contiguous_clip_span(&spans, b).unwrap();
        assert_eq!(ids, vec![a, b, c]);
        assert_eq!((start, end), (0, 30));

        let (ids, start, end) = contiguous_clip_span(&spans, d).unwrap();
        assert_eq!(ids, vec![d]);
        assert_eq!((start, end), (40, 50));
    }

    #[test]
    fn templates_create_expected_tracks() {
        let band = project_from_template(Template::Band);
        let names: Vec<&str> = band
            .track_order
            .iter()
            .filter_map(|id| band.tracks.get(id).map(|track| track.name.as_str()))
            .collect();
        assert_eq!(names, vec!["Drums", "Bass", "Guitar", "Keys", "Vocals"]);

        let scoring = project_from_template(Template::Scoring);
        assert_eq!(scoring.track_order.len(), 4);

        let empty = project_from_template(Template::Empty);
        assert_eq!(empty.track_order.len(), 1);
    }

    #[test]
    fn autosave_sits_next_to_the_project() {
        let path = std::path::PathBuf::from("/tmp/song.zoft");
        assert_eq!(
            super::AppUI::sibling_autosave(&path),
            std::path::PathBuf::from("/tmp/song.autosave.zoft")
        );
    }

    #[test]
    fn group_move_stops_at_the_timeline_start() {
        assert_eq!(clamp_group_delta(100, -20), -20);
        assert_eq!(clamp_group_delta(100, -150), -100);
        assert_eq!(clamp_group_delta(100, 50), 50);
        assert_eq!(clamp_group_delta(0, -10), 0);
    }

    #[test]
    fn clip_span_converts_to_project_frames() {
        let source = AudioFileRef::new(
            std::path::PathBuf::from("a.wav"),
            48_000,
            48_000,
            2,
            BitDepth::Float32,
        );
        let clip = AudioClip::new(
            source,
            TimelinePos::from_sample(1_000, 48_000.0, 120.0),
            24_000,
        );
        assert_eq!(clip_span_samples(&clip, 48_000.0), (1_000, 25_000));

        // A 96 kHz source is half as long in 48 kHz project frames.
        let source = AudioFileRef::new(
            std::path::PathBuf::from("b.wav"),
            96_000,
            96_000,
            2,
            BitDepth::Float32,
        );
        let clip = AudioClip::new(
            source,
            TimelinePos::from_sample(1_000, 48_000.0, 120.0),
            96_000,
        );
        assert_eq!(clip_span_samples(&clip, 48_000.0), (1_000, 49_000));
    }

    #[test]
    fn take_lane_layout_stacks_significant_overlaps() {
        let mut project = Project::new("takes");
        let sample_rate = 48_000u32;
        let source = AudioFileRef::new(
            std::path::PathBuf::from("take.wav"),
            48_000,
            sample_rate,
            2,
            BitDepth::Float32,
        );
        let mut make = |project: &mut Project, sample: u64| {
            let clip = AudioClip::new(
                source.clone(),
                TimelinePos::from_sample(sample, sample_rate as f64, 120.0),
                4_800,
            );
            project.audio_clips.insert(clip)
        };
        let first = make(&mut project, 0);
        let second = make(&mut project, 2_400); // 50% overlap
        let third = make(&mut project, 100_000); // separate
        let ids = vec![first, second, third];
        let layout = take_lane_layout(&project, &ids);

        assert_eq!(layout.get(&first), Some(&(0, 2)));
        assert_eq!(layout.get(&second), Some(&(1, 2)));
        assert_eq!(layout.get(&third).copied().unwrap_or((0, 1)), (0, 1));
    }

    #[test]
    fn marquee_selects_intersecting_clips() {
        let mut project = Project::new("marquee");
        let source = AudioFileRef::new(
            std::path::PathBuf::from("source.wav"),
            100,
            48_000,
            1,
            BitDepth::Float32,
        );
        let template = AudioClip::new(source, TimelinePos::default(), 1);
        let a = project.audio_clips.insert(template.clone());
        let b = project.audio_clips.insert(template.clone());

        let rects = [
            (
                a,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)),
            ),
            (
                b,
                egui::Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(30.0, 10.0)),
            ),
        ];

        let hits = clips_in_marquee(
            &rects,
            egui::Rect::from_min_max(egui::pos2(5.0, 0.0), egui::pos2(25.0, 10.0)),
        );
        assert_eq!(hits, vec![a, b]);

        let hits = clips_in_marquee(
            &rects,
            egui::Rect::from_min_max(egui::pos2(-5.0, -5.0), egui::pos2(1.0, 1.0)),
        );
        assert_eq!(hits, vec![a]);
    }

    #[test]
    fn midi_marquee_selects_intersecting_clips() {
        let mut project = Project::new("midi marquee");
        let template = MidiClip::new(TimelinePos::default(), 960);
        let a = project.midi_clips.insert(template.clone());
        let b = project.midi_clips.insert(template);

        let rects = [
            (
                a,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)),
            ),
            (
                b,
                egui::Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(30.0, 10.0)),
            ),
        ];

        let hits = midi_clips_in_marquee(
            &rects,
            egui::Rect::from_min_max(egui::pos2(5.0, 0.0), egui::pos2(25.0, 10.0)),
        );
        assert_eq!(hits, vec![a, b]);
    }

    #[test]
    fn note_marquee_selects_intersecting_notes() {
        let rects = [
            (
                0,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)),
            ),
            (
                1,
                egui::Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(30.0, 10.0)),
            ),
            (
                2,
                egui::Rect::from_min_max(egui::pos2(0.0, 20.0), egui::pos2(10.0, 30.0)),
            ),
        ];
        let hits = notes_in_marquee(
            &rects,
            egui::Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(25.0, 15.0)),
        );
        assert_eq!(hits, std::collections::BTreeSet::from([0, 1]));
    }

    #[test]
    fn velocity_colours_span_blue_to_yellow() {
        let low = velocity_color(10);
        let high = velocity_color(127);
        assert!(low.b() > low.r());
        assert!(high.r() > high.b());
        assert_ne!(low, high);
    }

    #[test]
    fn tape_stretch_changes_length_and_preserves_channels() {
        let input = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0];

        let longer = tape_stretch_interleaved(&input, 2, 2.0);
        assert_eq!(longer.len(), 16);
        assert_eq!(&longer[0..2], &[1.0, 10.0]);
        assert_eq!(&longer[4..6], &[2.0, 20.0]);
        assert_eq!(&longer[8..10], &[3.0, 30.0]);
        assert_eq!(&longer[12..14], &[4.0, 40.0]);

        let shorter = tape_stretch_interleaved(&input, 2, 0.5);
        assert_eq!(shorter.len(), 4);
        assert_eq!(&shorter[0..2], &[1.0, 10.0]);
        assert_eq!(&shorter[2..4], &[3.0, 30.0]);
    }

    #[test]
    fn integrated_loudness_tracks_signal_level() {
        let sine = |sample_rate: usize, amplitude: f32| {
            let frames = sample_rate;
            let mut samples = Vec::with_capacity(frames * 2);
            for frame in 0..frames {
                let t = frame as f32 / sample_rate as f32;
                let sample = amplitude * (std::f32::consts::TAU * 1_000.0 * t).sin();
                samples.push(sample);
                samples.push(sample);
            }
            samples
        };

        let quiet = measure_integrated_loudness(&sine(48_000, 0.1), 2, 48_000)
            .expect("sine should have measurable loudness");
        let quiet_44k = measure_integrated_loudness(&sine(44_100, 0.1), 2, 44_100)
            .expect("sine should have measurable loudness");
        let loud = measure_integrated_loudness(&sine(48_000, 0.2), 2, 48_000)
            .expect("sine should have measurable loudness");

        // A −20 dBFS 1 kHz stereo sine measures ≈ −20 LUFS (also confirmed
        // against ffmpeg's ebur128 filter).
        assert!((quiet.integrated_lufs + 20.0).abs() < 0.1);
        assert!((quiet_44k.integrated_lufs - quiet.integrated_lufs).abs() < 0.05);
        assert!((quiet.peak - 0.1).abs() < 1e-3);
        assert!((loud.integrated_lufs - quiet.integrated_lufs - 6.0206).abs() < 0.05);
        assert!(measure_integrated_loudness(&vec![0.0; 48_000 * 2], 2, 48_000).is_none());
    }

    #[test]
    fn bit_crusher_holds_and_quantizes_samples() {
        let mut samples = vec![0.6, 0.7, -0.6, 0.8];
        bit_crush(&mut samples, 1, 1, 2);
        assert_eq!(samples, vec![1.0, 1.0, -1.0, -1.0]);
    }

    #[test]
    fn saturation_mix_zero_is_dry() {
        let original = vec![0.25, -0.5, 0.9];
        let mut samples = original.clone();
        saturate(&mut samples, 24.0, 0.0);
        assert_eq!(samples, original);

        let mut wet = original;
        saturate(&mut wet, 24.0, 1.0);
        assert!(wet.iter().all(|sample| sample.abs() <= 1.0 + 1e-6));
        assert!(wet[0] > 0.25);
    }

    #[test]
    fn filters_low_and_high_pass() {
        let mut low = vec![1.0; 100];
        filter_audio(&mut low, 1, 1_000, 100.0, FilterKind::LowPass);
        assert!(low[0] > 0.0 && low[0] < 1.0);
        assert!(low[99] > 0.99);

        let mut high = vec![1.0; 100];
        filter_audio(&mut high, 1, 1_000, 100.0, FilterKind::HighPass);
        assert!(high[0] > 0.0 && high[0] < 1.0);
        assert!(high[99].abs() < 0.01);
    }

    #[test]
    fn tremolo_modulates_volume_at_the_lfo_rate() {
        let mut samples = vec![1.0; 1_001];
        tremolo(&mut samples, 1, 1_000, 1.0, 1.0);
        assert!((samples[0] - 1.0).abs() < 1e-6);
        assert!(samples[500].abs() < 1e-6);
        assert!((samples[1000] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn load_wav_range_reads_selected_frames() {
        let input = std::env::temp_dir().join(format!(
            "zoft_load_range_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&input, spec).unwrap();
        for frame in [[0.1f32, -0.2], [0.3, -0.4], [0.5, -0.6]] {
            writer.write_sample(frame[0]).unwrap();
            writer.write_sample(frame[1]).unwrap();
        }
        writer.finalize().unwrap();

        let (samples, channels, sample_rate) =
            load_wav_range_interleaved(&input, 1, 2).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(sample_rate, 48_000);
        assert_eq!(samples.len(), 4);
        for (sample, expected) in samples.iter().zip([0.3f32, -0.4, 0.5, -0.6]) {
            assert!((sample - expected).abs() < 1e-6);
        }

        std::fs::remove_file(input).ok();
    }

    #[test]
    fn write_wav_f32_round_trips_samples() {
        let path = std::env::temp_dir().join(format!(
            "zoft_write_f32_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let expected = vec![0.25f32, -0.5, 0.75];
        write_wav_f32(&path, &expected, 1, 48_000).unwrap();

        let (samples, channels, sample_rate) = load_wav_interleaved(&path).unwrap();
        assert_eq!(channels, 1);
        assert_eq!(sample_rate, 48_000);
        assert_eq!(samples, expected);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn write_wav_range_applies_frame_envelope() {
        let input = std::env::temp_dir().join(format!(
            "zoft_env_in_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let output = std::env::temp_dir().join(format!(
            "zoft_env_out_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&input, spec).unwrap();
        for _ in 0..4 {
            writer.write_sample(1.0f32).unwrap();
        }
        writer.finalize().unwrap();

        let envelope = [0.0f32, 0.5, 0.25, 1.0];
        write_wav_range_with_envelope(&input, 0, 4, &output, |frame| {
            envelope[frame as usize]
        })
        .unwrap();

        let (samples, _, _) = load_wav_interleaved(&output).unwrap();
        assert_eq!(samples.len(), 4);
        for (sample, expected) in samples.iter().zip(envelope) {
            assert!((sample - expected).abs() < 1e-6);
        }

        std::fs::remove_file(input).ok();
        std::fs::remove_file(output).ok();
    }

    #[test]
    fn rubberband_renders_time_stretched_audio() {
        let available = std::process::Command::new("rubberband")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !available {
            return;
        }

        let input = std::env::temp_dir().join(format!(
            "zoft_rb_in_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let output = std::env::temp_dir().join(format!(
            "zoft_rb_out_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&input, spec).unwrap();
        for frame in 0..4_800 {
            let value =
                (frame as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();

        render_pitch_stretch(
            &input,
            &output,
            0.0,
            1.5,
            false,
            TimeStretchMode::Elastic,
        )
        .unwrap();
        let (frames, _) = wav_file_info(&output).unwrap();
        assert!((frames as f64 - 7_200.0).abs() <= 2.0);

        std::fs::remove_file(input).ok();
        std::fs::remove_file(output).ok();
    }

    #[test]
    fn wav_peak_reads_only_the_requested_range() {
        let path = std::env::temp_dir().join(format!(
            "zoft_peak_{}.wav",
            uuid::Uuid::new_v4()
        ));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for frame in [
            [0.1, -0.2],
            [-0.9, 0.3],
            [0.2, 0.25],
            [0.5, -0.6],
        ] {
            writer.write_sample(frame[0]).unwrap();
            writer.write_sample(frame[1]).unwrap();
        }
        writer.finalize().unwrap();

        // Frames 1..3 contain the -0.9 peak; the later -0.6 and 0.5 are outside.
        let peak = wav_peak(&path, 1, 2).unwrap();
        assert!((peak - 0.9).abs() < 1e-6);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn midi_clip_note_spans_pair_note_on_and_off() {
        let mut clip = MidiClip::new(TimelinePos::default(), 1920);
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));
        clip.add_event(MidiEvent::note_off(480, 0, 60, 0));
        clip.add_event(MidiEvent::note_on(960, 0, 64, 80));
        clip.add_event(MidiEvent::note_off(1440, 0, 64, 0));

        let spans = midi_clip_note_spans(&clip);
        assert_eq!(spans, vec![(0, 480, 60, 100), (960, 1440, 64, 80)]);
    }

    #[test]
    fn midi_clip_note_spans_close_held_notes_at_the_clip_end() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.add_event(MidiEvent::note_on(480, 0, 67, 90));

        let spans = midi_clip_note_spans(&clip);
        assert_eq!(spans, vec![(480, 960, 67, 90)]);
    }

    #[test]
    fn midi_clip_notes_keep_channels_and_overlaps() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));
        clip.add_event(MidiEvent::note_on(120, 0, 60, 90));
        clip.add_event(MidiEvent::note_off(480, 0, 60, 0));
        clip.add_event(MidiEvent::note_off(600, 0, 60, 0));
        clip.add_event(MidiEvent::note_on(240, 1, 64, 70));
        clip.add_event(MidiEvent::note_off(720, 1, 64, 0));

        let notes = midi_clip_notes(&clip);
        assert_eq!(
            notes,
            vec![
                PianoRollNote {
                    start: 0,
                    end: 480,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 120,
                    end: 600,
                    pitch: 60,
                    velocity: 90,
                    channel: 0,
                },
                PianoRollNote {
                    start: 240,
                    end: 720,
                    pitch: 64,
                    velocity: 70,
                    channel: 1,
                },
            ]
        );
    }

    #[test]
    fn piano_roll_notes_round_trip_and_keep_other_events() {
        let mut clip = MidiClip::new(TimelinePos::default(), 1920);
        clip.add_event(MidiEvent::control_change(0, 0, 1, 64));
        let notes = vec![
            PianoRollNote {
                start: 0,
                end: 480,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 960,
                end: 1440,
                pitch: 64,
                velocity: 80,
                channel: 0,
            },
        ];

        write_piano_roll_notes(&mut clip, &notes);
        assert_eq!(midi_clip_notes(&clip), notes);
        assert!(clip.events.iter().any(|event| matches!(
            event.message,
            daw_core::clip::MidiMessage::ControlChange { controller: 1, value: 64, .. }
        )));
    }

    #[test]
    fn piano_roll_snap_ticks_follow_the_header_choice() {        assert_eq!(PianoRollSnap::Off.ticks(4), 1);
        assert_eq!(PianoRollSnap::Bar.ticks(4), 3840);
        assert_eq!(PianoRollSnap::Beat.ticks(4), 960);
        assert_eq!(PianoRollSnap::Eighth.ticks(4), 480);
        assert_eq!(PianoRollSnap::Sixteenth.ticks(4), 240);
        assert_eq!(PianoRollSnap::ThirtySecond.ticks(4), 120);
        assert_eq!(PianoRollSnap::EighthTriplet.ticks(4), 320);
        assert_eq!(PianoRollSnap::SixteenthTriplet.ticks(4), 160);
    }

    #[test]
    fn piano_roll_new_note_length_uses_the_snap() {
        assert_eq!(
            piano_roll_new_note_length(PianoRollSnap::Sixteenth, 240, 960),
            240
        );
        assert_eq!(
            piano_roll_new_note_length(PianoRollSnap::EighthTriplet, 320, 960),
            320
        );
        assert_eq!(
            piano_roll_new_note_length(PianoRollSnap::Off, 1, 960),
            960
        );
    }

    #[test]
    fn midi_quantize_preserves_duration_and_strength() {
        let mut notes = vec![PianoRollNote {
            start: 100,
            end: 400,
            pitch: 60,
            velocity: 100,
            channel: 0,
        }];
        quantize_midi_notes(&mut notes, 240, 1.0, 0.0);
        assert_eq!(notes[0].start, 0);
        assert_eq!(notes[0].end, 300);

        notes[0].start = 100;
        notes[0].end = 400;
        quantize_midi_notes(&mut notes, 240, 0.5, 0.0);
        assert_eq!(notes[0].start, 50);
        assert_eq!(notes[0].end, 350);
    }

    #[test]
    fn midi_quantize_swing_delays_offbeat_positions() {
        let make = || {
            vec![
                PianoRollNote {
                    start: 100,
                    end: 340,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 500,
                    end: 740,
                    pitch: 62,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 1000,
                    end: 1240,
                    pitch: 64,
                    velocity: 100,
                    channel: 0,
                },
            ]
        };

        let mut notes = make();
        quantize_midi_notes(&mut notes, 480, 1.0, 1.0);
        assert_eq!(
            notes.iter().map(|note| note.start).collect::<Vec<_>>(),
            vec![0, 640, 960]
        );

        let mut notes = make();
        quantize_midi_notes(&mut notes, 480, 1.0, 0.5);
        assert_eq!(
            notes.iter().map(|note| note.start).collect::<Vec<_>>(),
            vec![0, 560, 960]
        );
    }

    #[test]
    fn midi_transpose_and_velocity_clamp_to_valid_ranges() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 120,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 240,
                end: 360,
                pitch: 120,
                velocity: 10,
                channel: 0,
            },
        ];
        transpose_midi_notes(&mut notes, 12);
        assert_eq!(notes[0].pitch, 72);
        assert_eq!(notes[1].pitch, 127);
        transpose_midi_notes(&mut notes, -100);
        assert_eq!(notes[0].pitch, 0);
        assert_eq!(notes[1].pitch, 27);

        scale_midi_velocities(&mut notes, 2.0);
        assert_eq!(notes[0].velocity, 127);
        scale_midi_velocities(&mut notes, 0.0);
        assert_eq!(notes[0].velocity, 1);
    }

    #[test]
    fn velocity_shaping_modes_compress_expand_and_curve() {
        let make = || {
            vec![
                PianoRollNote {
                    start: 0,
                    end: 240,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 240,
                    end: 480,
                    pitch: 64,
                    velocity: 20,
                    channel: 0,
                },
            ]
        };

        let mut notes = make();
        compress_midi_velocities(&mut notes, 1.0);
        assert_eq!(
            notes.iter().map(|note| note.velocity).collect::<Vec<_>>(),
            vec![64, 64]
        );

        let mut notes = make();
        expand_midi_velocities(&mut notes, 1.0);
        assert_eq!(
            notes.iter().map(|note| note.velocity).collect::<Vec<_>>(),
            vec![127, 1]
        );

        let mut notes = make();
        curve_midi_velocities(&mut notes, 1.0);
        assert!(notes[0].velocity > 100);
        assert!(notes[1].velocity > 20);
        for note in &notes {
            assert_eq!(note.end - note.start, 240);
        }

        let mut notes = make();
        curve_midi_velocities(&mut notes, -1.0);
        assert!(notes[0].velocity < 100);
        assert!(notes[1].velocity < 20);
    }

    #[test]
    fn scale_conform_maps_to_nearest_scale_degree() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 120,
                pitch: 61,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 120,
                end: 240,
                pitch: 63,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 240,
                end: 360,
                pitch: 66,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 360,
                end: 480,
                pitch: 71,
                velocity: 100,
                channel: 0,
            },
        ];
        scale_conform_midi_notes(&mut notes, 0, &[0, 2, 4, 5, 7, 9, 11]);
        assert_eq!(
            notes.iter().map(|note| note.pitch).collect::<Vec<_>>(),
            vec![60, 62, 65, 71]
        );

        scale_conform_midi_notes(&mut notes, 0, &[0, 3, 5, 7, 10]);
        assert_eq!(
            notes.iter().map(|note| note.pitch).collect::<Vec<_>>(),
            vec![60, 63, 65, 70]
        );
    }

    #[test]
    fn humanize_preserves_durations_and_stays_in_range() {
        let mut notes = vec![
            PianoRollNote {
                start: 1000,
                end: 1480,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 2000,
                end: 2480,
                pitch: 64,
                velocity: 60,
                channel: 0,
            },
        ];
        humanize_midi_notes(&mut notes, 120, 20, 42);
        for note in &notes {
            assert_eq!(note.end - note.start, 480);
            assert!(note.velocity >= 1 && note.velocity <= 127);
            assert!((note.start as i64 - 1000).abs() <= 120 || (note.start as i64 - 2000).abs() <= 120);
        }
    }

    #[test]
    fn legato_extends_notes_toward_the_next_start() {
        let make = || {
            vec![
                PianoRollNote {
                    start: 0,
                    end: 240,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 480,
                    end: 720,
                    pitch: 62,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 960,
                    end: 1200,
                    pitch: 64,
                    velocity: 100,
                    channel: 0,
                },
            ]
        };

        let mut notes = make();
        legato_midi_notes(&mut notes, 0);
        assert_eq!(
            notes.iter().map(|note| note.end).collect::<Vec<_>>(),
            vec![480, 960, 1200]
        );

        let mut notes = make();
        legato_midi_notes(&mut notes, 120);
        assert_eq!(
            notes.iter().map(|note| note.end).collect::<Vec<_>>(),
            vec![360, 840, 1200]
        );
    }

    #[test]
    fn set_note_length_sets_every_note_end() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 240,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 480,
                end: 720,
                pitch: 64,
                velocity: 100,
                channel: 0,
            },
        ];
        set_midi_note_length(&mut notes, 360);
        assert_eq!(notes[0].end, 360);
        assert_eq!(notes[1].end, 840);
    }

    #[test]
    fn join_midi_notes_merges_same_pitch_and_respects_gap() {
        let make = || {
            vec![
                PianoRollNote {
                    start: 0,
                    end: 240,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 240,
                    end: 480,
                    pitch: 60,
                    velocity: 80,
                    channel: 0,
                },
                PianoRollNote {
                    start: 600,
                    end: 840,
                    pitch: 60,
                    velocity: 70,
                    channel: 0,
                },
                PianoRollNote {
                    start: 240,
                    end: 480,
                    pitch: 64,
                    velocity: 90,
                    channel: 0,
                },
                PianoRollNote {
                    start: 240,
                    end: 480,
                    pitch: 60,
                    velocity: 60,
                    channel: 1,
                },
            ]
        };

        let mut notes = make();
        join_midi_notes(&mut notes, 0);
        assert_eq!(notes.len(), 4);
        let joined = notes
            .iter()
            .find(|note| {
                note.pitch == 60 && note.channel == 0 && note.start == 0
            })
            .expect("joined note");
        assert_eq!((joined.start, joined.end, joined.velocity), (0, 480, 100));

        let mut notes = make();
        join_midi_notes(&mut notes, 120);
        assert_eq!(notes.len(), 3);
        let joined = notes
            .iter()
            .find(|note| {
                note.pitch == 60 && note.channel == 0 && note.start == 0
            })
            .expect("joined note");
        assert_eq!((joined.start, joined.end), (0, 840));
    }

    #[test]
    fn split_midi_notes_at_tick_duplicates_crossing_notes() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 960,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 480,
                end: 1440,
                pitch: 64,
                velocity: 80,
                channel: 0,
            },
            PianoRollNote {
                start: 960,
                end: 1200,
                pitch: 67,
                velocity: 90,
                channel: 0,
            },
        ];
        split_midi_notes_at_tick(&mut notes, 960);
        assert_eq!(
            notes
                .iter()
                .map(|note| (note.start, note.end, note.pitch))
                .collect::<Vec<_>>(),
            vec![
                (0, 960, 60),
                (480, 960, 64),
                (960, 1440, 64),
                (960, 1200, 67),
            ]
        );
        assert_eq!(notes[2].velocity, 80);

        let before = notes.clone();
        split_midi_notes_at_tick(&mut notes, 960);
        assert_eq!(notes, before);
    }

    #[test]
    fn ghost_notes_include_other_tracks_and_filter_outside_clip() {
        let mut project = Project::new("Ghosts");
        let mut track = daw_core::track::Track::new_midi("Ghost track");
        let mut current = MidiClip::new(TimelinePos::new(960, 0), 1920);
        current.content_offset = 480;
        let current_id = project.midi_clips.insert(current);
        track.add_midi_clip(current_id);

        let mut other = MidiClip::new(TimelinePos::default(), 1920);
        other.add_event(MidiEvent::note_on(1200, 0, 60, 100));
        other.add_event(MidiEvent::note_off(1440, 0, 60, 0));
        let other_id = project.midi_clips.insert(other);
        track.add_midi_clip(other_id);

        let mut different_track =
            daw_core::track::Track::new_midi("Different track");
        let mut other_track_clip = MidiClip::new(TimelinePos::default(), 1920);
        other_track_clip.add_event(MidiEvent::note_on(1200, 0, 67, 100));
        other_track_clip.add_event(MidiEvent::note_off(1440, 0, 67, 0));
        let other_track_id = project.midi_clips.insert(other_track_clip);
        different_track.add_midi_clip(other_track_id);

        let mut far_track = daw_core::track::Track::new_midi("Far track");
        let mut far_clip = MidiClip::new(TimelinePos::default(), 1920);
        far_clip.add_event(MidiEvent::note_on(5000, 0, 72, 100));
        far_clip.add_event(MidiEvent::note_off(5240, 0, 72, 0));
        let far_id = project.midi_clips.insert(far_clip);
        far_track.add_midi_clip(far_id);

        project.add_track(track);
        let different_track_id = project.add_track(different_track);
        project.add_track(far_track);

        let mut ghosts = ghost_notes_for_clip(&project, current_id, 1920);
        ghosts.sort_by_key(|note| note.pitch);
        assert_eq!(ghosts.len(), 2);
        assert_eq!(
            (ghosts[0].start, ghosts[0].end, ghosts[0].pitch),
            (720, 960, 60)
        );
        assert_eq!(
            (ghosts[1].start, ghosts[1].end, ghosts[1].pitch),
            (720, 960, 67)
        );

        project
            .tracks
            .get_mut(&different_track_id)
            .unwrap()
            .show_as_ghost = false;
        let ghosts = ghost_notes_for_clip(&project, current_id, 1920);
        assert_eq!(ghosts.len(), 1);
        assert_eq!(ghosts[0].pitch, 60);
    }

    #[test]
    fn thin_midi_notes_keeps_a_seeded_subset() {
        let make = || {
            (0..8)
                .map(|index| PianoRollNote {
                    start: index * 240,
                    end: index * 240 + 120,
                    pitch: 60 + index as u8,
                    velocity: 100,
                    channel: 0,
                })
                .collect::<Vec<_>>()
        };

        let mut notes = make();
        thin_midi_notes(&mut notes, 1.0, 7);
        assert_eq!(notes, make());

        let mut notes = make();
        thin_midi_notes(&mut notes, 0.0, 7);
        assert!(notes.is_empty());

        let mut first = make();
        thin_midi_notes(&mut first, 0.5, 42);
        let mut second = make();
        thin_midi_notes(&mut second, 0.5, 42);
        assert_eq!(first, second);
        let original = make();
        assert!(first
            .iter()
            .all(|note| original.iter().any(|source| source == note)));
    }

    #[test]
    fn reverse_midi_notes_flips_positions_and_keeps_durations() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 480,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 960,
                end: 1440,
                pitch: 64,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 1920,
                end: 2400,
                pitch: 67,
                velocity: 100,
                channel: 0,
            },
        ];
        reverse_midi_notes(&mut notes, 2400);
        assert_eq!(
            notes
                .iter()
                .map(|note| (note.start, note.end, note.pitch))
                .collect::<Vec<_>>(),
            vec![(1920, 2400, 60), (960, 1440, 64), (0, 480, 67)]
        );
    }

    #[test]
    fn mirror_midi_notes_reflects_around_center_and_clamps() {
        let mut notes = vec![
            PianoRollNote {
                start: 0,
                end: 240,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 240,
                end: 480,
                pitch: 64,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 480,
                end: 720,
                pitch: 72,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 720,
                end: 960,
                pitch: 0,
                velocity: 100,
                channel: 0,
            },
        ];
        mirror_midi_notes(&mut notes, 60);
        assert_eq!(
            notes.iter().map(|note| note.pitch).collect::<Vec<_>>(),
            vec![60, 56, 48, 120]
        );
    }

    #[test]
    fn randomize_midi_notes_is_seeded_and_stays_in_range() {
        let make = || {
            vec![
                PianoRollNote {
                    start: 0,
                    end: 240,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 480,
                    end: 720,
                    pitch: 64,
                    velocity: 90,
                    channel: 0,
                },
                PianoRollNote {
                    start: 960,
                    end: 1200,
                    pitch: 67,
                    velocity: 80,
                    channel: 0,
                },
            ]
        };

        let mut first = make();
        randomize_midi_notes(&mut first, 2, 10, 60, 99);
        let mut second = make();
        randomize_midi_notes(&mut second, 2, 10, 60, 99);
        assert_eq!(first, second);

        for (index, note) in first.iter().enumerate() {
            assert_eq!(note.end - note.start, 240);
            let original = &make()[index];
            assert!((note.pitch as i16 - original.pitch as i16).abs() <= 2);
            assert!(
                (note.velocity as i16 - original.velocity as i16).abs() <= 10
            );
            assert!(
                (note.start as i64 - original.start as i64).abs() <= 60
            );
            assert!(note.velocity >= 1 && note.velocity <= 127);
        }
    }

    #[test]
    fn piano_roll_fit_contains_the_clip_and_notes() {
        let mut window = PianoRollWindow::new(ClipId::default(), SnapMode::Beat);
        window.notes = vec![PianoRollNote {
            start: 0,
            end: 480,
            pitch: 72,
            velocity: 100,
            channel: 0,
        }];
        let grid = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(800.0, 400.0),
        );
        fit_piano_roll_view(&mut window, grid, 7680);

        assert!(window.pixels_per_tick > 0.0);
        assert!(window.view_start_tick >= 0.0);
        assert!(window.view_low_pitch <= 72.0);
        assert!(
            window.view_low_pitch + 400.0 / window.pixels_per_semitone >= 72.0
        );
    }

    #[test]
    fn piano_roll_fit_selection_frames_selected_notes() {
        let mut window = PianoRollWindow::new(ClipId::default(), SnapMode::Beat);
        window.notes = vec![
            PianoRollNote {
                start: 0,
                end: 480,
                pitch: 60,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 960,
                end: 1440,
                pitch: 72,
                velocity: 100,
                channel: 0,
            },
            PianoRollNote {
                start: 1920,
                end: 2400,
                pitch: 48,
                velocity: 100,
                channel: 0,
            },
        ];
        window.selected.insert(1);
        let grid = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(800.0, 400.0),
        );
        fit_piano_roll_selection(&mut window, grid);

        assert!(window.pixels_per_tick > 0.0);
        assert!(window.view_start_tick <= 960.0);
        let visible_ticks = 800.0 / window.pixels_per_tick;
        assert!(window.view_start_tick + visible_ticks as f64 >= 1440.0);
        assert!(window.view_low_pitch <= 72.0);
        let visible_pitches = 400.0 / window.pixels_per_semitone;
        assert!(window.view_low_pitch + visible_pitches >= 72.0);
    }

    #[test]
    fn track_reorder_description_is_named() {
        let action = EditAction::MoveTrack {
            track_id: daw_core::track::TrackId::default(),
            from_index: 0,
            to_index: 1,
        };
        assert_eq!(action.description(), "Reorder track");
    }

    #[test]
    fn midi_clip_edit_description_is_named() {
        let clip = MidiClip::new(TimelinePos::default(), 960);
        let action = EditAction::MidiClip {
            clip_id: ClipId::default(),
            from: clip.clone(),
            to: clip,
        };
        assert_eq!(action.description(), "Edit MIDI clip");

        let moved = EditAction::MoveMidi {
            clip_id: ClipId::default(),
            from_track: daw_core::track::TrackId::default(),
            from_pos: TimelinePos::default(),
            to_track: daw_core::track::TrackId::default(),
            to_pos: TimelinePos::default(),
        };
        assert_eq!(moved.description(), "Move MIDI clip");
    }

    #[test]
    fn midi_clip_notes_respect_content_offset() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.content_offset = 480;
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));
        clip.add_event(MidiEvent::note_off(480, 0, 60, 0));
        clip.add_event(MidiEvent::note_on(480, 0, 64, 100));
        clip.add_event(MidiEvent::note_off(960, 0, 64, 0));
        clip.add_event(MidiEvent::note_on(960, 0, 67, 100));
        clip.add_event(MidiEvent::note_off(1440, 0, 67, 0));

        assert_eq!(
            midi_clip_notes(&clip),
            vec![
                PianoRollNote {
                    start: 0,
                    end: 480,
                    pitch: 64,
                    velocity: 100,
                    channel: 0,
                },
                PianoRollNote {
                    start: 480,
                    end: 960,
                    pitch: 67,
                    velocity: 100,
                    channel: 0,
                },
            ]
        );
    }

    #[test]
    fn writing_piano_roll_notes_keeps_trimmed_material() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.content_offset = 480;
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));
        clip.add_event(MidiEvent::note_off(480, 0, 60, 0));
        clip.add_event(MidiEvent::note_on(480, 0, 64, 100));
        clip.add_event(MidiEvent::note_off(960, 0, 64, 0));
        clip.add_event(MidiEvent::note_on(1920, 0, 67, 100));
        clip.add_event(MidiEvent::note_off(2400, 0, 67, 0));

        let mut notes = midi_clip_content_notes(&clip);
        assert_eq!(notes.len(), 3);
        let visible = notes
            .iter_mut()
            .find(|note| note.pitch == 64)
            .expect("visible note");
        visible.pitch = 65;
        write_piano_roll_notes(&mut clip, &notes);

        let content = midi_clip_content_notes(&clip);
        assert!(content.iter().any(|note| note.pitch == 60));
        assert!(content.iter().any(|note| note.pitch == 65));
        assert!(content.iter().any(|note| note.pitch == 67));
    }

    #[test]
    fn timeline_snap_ticks_follow_the_arrange_grid() {
        assert_eq!(timeline_snap_ticks(SnapMode::Off, 4), 1);
        assert_eq!(timeline_snap_ticks(SnapMode::Bar, 4), 3840);
        assert_eq!(timeline_snap_ticks(SnapMode::Beat, 4), 960);
        assert_eq!(timeline_snap_ticks(SnapMode::Eighth, 4), 480);
        assert_eq!(timeline_snap_ticks(SnapMode::Sixteenth, 4), 240);
    }

    #[test]
    fn midi_trim_start_moves_the_visible_window() {
        let mut clip = MidiClip::new(TimelinePos::new(960, 0), 960);
        clip.content_offset = 480;

        assert_eq!(midi_trim_start(&clip, 1200, 240), (1200, 720, 720));
        assert_eq!(midi_trim_start(&clip, 720, 240), (720, 240, 1200));
        // Cannot trim before the recorded content begins.
        assert_eq!(midi_trim_start(&clip, 0, 240), (480, 0, 1440));
    }

    #[test]
    fn midi_trim_end_changes_visible_length() {
        let clip = MidiClip::new(TimelinePos::new(960, 0), 960);
        assert_eq!(midi_trim_end(&clip, 1440, 240), 480);
        assert_eq!(midi_trim_end(&clip, 1920, 240), 960);
        // At least one tick remains visible.
        assert_eq!(midi_trim_end(&clip, 0, 240), 1);
    }

    #[test]
    fn splitting_midi_clip_preserves_content_windows() {
        let mut clip = MidiClip::new(TimelinePos::new(960, 0), 1920);
        clip.content_offset = 240;

        let (left, right) =
            split_midi_clip_at_tick(&clip, 1920).expect("split inside clip");
        assert_eq!(left.length, 960);
        assert_eq!(left.content_offset, 240);
        assert_eq!(right.timeline_position.tick, 1920);
        assert_eq!(right.content_offset, 1200);
        assert_eq!(right.length, 960);

        assert!(split_midi_clip_at_tick(&clip, 960).is_none());
        assert!(split_midi_clip_at_tick(&clip, 2880).is_none());
    }

    #[test]
    fn cc_points_round_trip_and_preserve_other_controllers() {
        let mut clip = MidiClip::new(TimelinePos::default(), 960);
        clip.add_event(MidiEvent::control_change(0, 0, 1, 64));
        clip.add_event(MidiEvent::control_change(240, 0, 1, 100));
        clip.add_event(MidiEvent::control_change(0, 0, 7, 90));
        clip.add_event(MidiEvent::note_on(0, 0, 60, 100));

        assert_eq!(
            midi_clip_cc_points(&clip, 1),
            vec![
                MidiCcPoint {
                    tick: 0,
                    value: 64,
                    channel: 0
                },
                MidiCcPoint {
                    tick: 240,
                    value: 100,
                    channel: 0
                },
            ]
        );

        let replacement = [MidiCcPoint {
            tick: 480,
            value: 20,
            channel: 0,
        }];
        write_midi_cc_points(&mut clip, 1, &replacement);
        assert_eq!(midi_clip_cc_points(&clip, 1), replacement.to_vec());
        assert_eq!(midi_clip_cc_points(&clip, 7).len(), 1);
        assert!(clip.events.iter().any(|event| matches!(
            event.message,
            daw_core::clip::MidiMessage::NoteOn { note: 60, .. }
        )));
    }

    #[test]
    fn note_names_use_scientific_pitch_notation() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(0), "C-1");
    }
}
