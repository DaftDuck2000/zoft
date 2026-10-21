//! Main application

use anyhow::Result;
use daw_core::audio_pool::AudioFileRef;
use daw_core::clip::{AudioClip, ClipId, Fade, FadeCurve, TimeStretchMode, TimelinePos};
use daw_core::project::{BitDepth, Color};
use daw_core::track::TrackId;
use daw_core::undo::History;
use daw_core::Project;
use daw_core::WaveformOverview;
use daw_engine::playback::{PlayableClip, PlaybackMixer};
use daw_engine::AudioEngine;
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
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
    }
}

/// Undoable track properties toggled from the track list.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TrackShape {
    mute: bool,
    solo: bool,
    record_arm: bool,
    volume: f32,
}

impl TrackShape {
    fn of(track: &daw_core::track::Track) -> Self {
        Self {
            mute: track.mute,
            solo: track.solo,
            record_arm: track.record_arm,
            volume: track.volume,
        }
    }

    fn apply(self, track: &mut daw_core::track::Track) {
        track.mute = self.mute;
        track.solo = self.solo;
        track.record_arm = self.record_arm;
        track.set_volume(self.volume);
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
    /// Toggle track mute, solo or record-arm.
    TrackState {
        track_id: TrackId,
        from: TrackShape,
        to: TrackShape,
    },
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
                if from.gain != to.gain {
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
            EditAction::TrackState { from, to, .. } => {
                if from.mute != to.mute {
                    if to.mute { "Mute track" } else { "Unmute track" }.to_string()
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
                } else {
                    "Track change".to_string()
                }
            }
            EditAction::Group { label, .. } => label.clone(),
        }
    }
}

/// Merge a continuous `Shape` edit into the most recent history entry when the
/// new edit starts exactly where the previous one ended (same clip). Returns
/// true when the edit was merged.
fn merge_continuous_edit(history: &mut History<EditAction>, action: &EditAction) -> bool {
    let EditAction::Shape { clip_id, from, to } = action else {
        return false;
    };
    let (clip_id, from, to) = (*clip_id, *from, *to);

    let Some(entry) = history.last_mut() else {
        return false;
    };
    let mut merged = false;
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
    const ALL: [Self; 11] = [
        Self::Normalize,
        Self::LufsNormalize,
        Self::BakeIntoClip,
        Self::Reverse,
        Self::PitchShift,
        Self::TimeStretch,
        Self::BitCrusher,
        Self::Saturation,
        Self::Filter,
        Self::Tremolo,
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

/// State of the floating per-clip audio-effects window.
#[derive(Debug, Clone)]
struct EffectsWindow {
    clip_id: ClipId,
    selected: EffectKind,
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

impl EffectsWindow {
    fn new(clip_id: ClipId) -> Self {
        Self {
            clip_id,
            selected: EffectKind::Normalize,
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

/// In-progress clip drag (move, trim or fade handle).
struct ClipDrag {
    clip_id: ClipId,
    mode: DragMode,
    from_shape: ClipShape,
    from_track: TrackId,
    last_track: TrackId,
    grab_offset_secs: f64,
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
    /// Where recording started, so the clip lands at the right spot.
    record_start_secs: Option<f64>,
    /// Armed take waiting for the transport to roll (see `begin_recording`).
    pending_record_track: Option<TrackId>,
    last_playhead_update: std::time::Instant,
    dragging_playhead: bool,
    input_devices: Vec<daw_engine::audio_backend::AudioDeviceInfo>,
    selected_input_device: Option<String>,
    error_message: Option<String>,

    // Timeline view state
    view_start_secs: f64,
    pixels_per_sec: f32,
    needs_fit: bool,
    last_timeline_width: f32,
    selected_clip: Option<ClipId>,

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
    clipboard: Option<AudioClip>,
    /// Floating audio-effects editor for the right-clicked clip.
    effects_window: Option<EffectsWindow>,
    /// Where a trashed clip was last attached, so Restore can put it back.
    deleted_from: HashMap<ClipId, TrackId>,
    history: History<EditAction>,
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
}

impl AppUI {
    fn new(project: Project, engine: AudioEngine) -> Self {
        let input_devices = engine.input_devices().unwrap_or_default();
        let (waveform_tx, waveform_rx) = mpsc::channel();

        let mut app = Self {
            project,
            engine,
            playing: false,
            recording: false,
            playhead_secs: 0.0,
            loop_enabled: false,
            loop_start_secs: 0.0,
            loop_end_secs: 0.0,
            record_start_secs: None,
            pending_record_track: None,
            last_playhead_update: std::time::Instant::now(),
            dragging_playhead: false,
            input_devices,
            selected_input_device: None,
            error_message: None,
            view_start_secs: 0.0,
            pixels_per_sec: 100.0,
            needs_fit: true,
            last_timeline_width: 1000.0,
            selected_clip: None,
            snap_mode: SnapMode::Beat,
            snap_relative: false,
            ruler_mode: RulerMode::BarsBeats,
            snap_suspended: false,
            drag: None,
            clipboard: None,
            effects_window: None,
            deleted_from: HashMap::new(),
            history: History::new(1000),
            merge_chain: false,
            history_open: false,
            needs_audio_refresh: false,
            waveform_tx,
            waveform_rx,
            detail_cache: DetailCache::default(),
        };

        // Projects loaded from disk may predate stored overviews; generate any
        // that are missing in the background.
        app.spawn_missing_overviews();
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
        if self.playing {
            let out_rate = self.engine.output_sample_rate().max(1);
            let clips = self.collect_playable_clips(out_rate);
            let start_frame = (self.playhead_secs.max(0.0) * out_rate as f64) as u64;
            self.sync_playback_loop();
            self.engine.set_playback_clips(clips, start_frame);
        }
    }

    fn push_undo(&mut self, action: EditAction, merge: bool) {
        // Merge continuous edits of the same clip (e.g. dragging a gain slider)
        // into a single history entry while the pointer stays down.
        if merge && self.merge_chain && merge_continuous_edit(&mut self.history, &action) {
            return;
        }

        let description = action.description();
        self.history.push(action, description);
        self.merge_chain = merge;
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
            EditAction::TrackState { track_id, from, to } => {
                if let Some(track) = self.project.tracks.get_mut(track_id) {
                    (if forward { *to } else { *from }).apply(track);
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
        if self.undo_step() {
            self.after_edit();
            self.refresh_playback_if_playing();
        }
    }

    fn redo(&mut self) {
        if self.redo_step() {
            self.after_edit();
            self.refresh_playback_if_playing();
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

    fn track_at_y(&self, y: f32, track_y_start: f32, track_height: f32) -> Option<TrackId> {
        if self.project.track_order.is_empty() || track_height <= 0.0 {
            return None;
        }
        let index = (((y - track_y_start) / track_height).floor() as i64)
            .clamp(0, self.project.track_order.len() as i64 - 1) as usize;
        self.project.track_order.get(index).copied()
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

        self.selected_clip = Some(clip_id);
        self.drag = Some(ClipDrag {
            clip_id,
            mode,
            from_shape: ClipShape::of(clip),
            from_track: track_id,
            last_track: track_id,
            grab_offset_secs: (view.t(rect, pos.x) - clip_start).max(0.0),
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
                if let Some(track_id) = self.track_at_y(pos.y, track_y_start, track_height) {
                    if track_id != drag.last_track {
                        self.detach_clip(drag.last_track, drag.clip_id);
                        self.attach_clip(track_id, drag.clip_id);
                        drag.last_track = track_id;
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
                if drag.last_track != drag.from_track
                    || current.position != drag.from_shape.position
                {
                    self.push_undo(
                        EditAction::Move {
                            clip_id: drag.clip_id,
                            from_track: drag.from_track,
                            from_pos: drag.from_shape.position,
                            to_track: drag.last_track,
                            to_pos: current.position,
                        },
                        false,
                    );
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

    /// Split the selected clip at the playhead.
    fn split_selected_clip(&mut self) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        self.split_clip_at(clip_id, self.playhead_secs);
    }

    /// Split `clip_id` at `split_secs` into two clips referencing the same
    /// source file. The audio file itself is never modified.
    fn split_clip_at(&mut self, clip_id: ClipId, split_secs: f64) {
        let Some(track_id) = self.clip_track(clip_id) else {
            return;
        };

        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked {
            return;
        }
        let src_sr = clip.source.sample_rate.max(1) as f64;
        let clip_start = clip.timeline_position.sample as f64 / project_sr;
        let clip_end = clip_start + clip.length as f64 / src_sr;

        if split_secs <= clip_start + MIN_CLIP_SECS || split_secs >= clip_end - MIN_CLIP_SECS {
            self.error_message =
                Some("The split point must be inside the clip.".to_string());
            return;
        }

        let from_shape = ClipShape::of(clip);
        let offset_frames = ((split_secs - clip_start) * src_sr).round().max(1.0) as u64;
        if clip.length <= offset_frames + 1 {
            return;
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
    }

    /// Delete the selected clip from its track. The clip data stays in the
    /// project pool, so the delete is recoverable (trash + undo).
    fn delete_selected_clip(&mut self) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let Some(track_id) = self.clip_track(clip_id) else {
            return;
        };
        if self
            .project
            .audio_clips
            .get(clip_id)
            .map(|clip| clip.locked)
            .unwrap_or(false)
        {
            return;
        }
        let action = EditAction::Detach { track: track_id, clip_id };
        self.apply_action(&action, true);
        self.push_undo(action, false);
        self.selected_clip = None;
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
        self.selected_clip = Some(clip_id);
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
        self.selected_clip = Some(new_id);
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
        if let Some(clip) = self
            .selected_clip
            .and_then(|id| self.project.audio_clips.get(id))
        {
            self.clipboard = Some(clip.clone());
        }
    }

    fn paste_clip(&mut self) {
        let Some(template) = self.clipboard.clone() else {
            return;
        };
        let track_id = self
            .selected_clip
            .and_then(|id| self.clip_track(id))
            .or_else(|| self.project.track_order.first().copied());
        let Some(track_id) = track_id else {
            self.error_message = Some("No track to paste into.".to_string());
            return;
        };

        let project_sr = self.project.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        let mut clip = template;
        clip.id = ClipId::default();
        clip.timeline_position = TimelinePos::from_sample(
            (self.playhead_secs.max(0.0) * project_sr).round() as u64,
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
        self.selected_clip = Some(new_id);
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    fn duplicate_selected_clip(&mut self) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let Some(track_id) = self.clip_track(clip_id) else {
            return;
        };
        let Some(clip) = self.project.audio_clips.get(clip_id) else {
            return;
        };
        if clip.locked {
            return;
        }

        let project_sr = self.project.sample_rate.max(1) as f64;
        let src_sr = clip.source.sample_rate.max(1) as f64;
        let bpm = self.project.tempo_map.default_bpm();
        let end_secs =
            clip.timeline_position.sample as f64 / project_sr + clip.length as f64 / src_sr;

        let mut copy = clip.clone();
        copy.id = ClipId::default();
        copy.timeline_position = TimelinePos::from_sample(
            (end_secs * project_sr).round() as u64,
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
        self.selected_clip = Some(new_id);
        self.after_edit();
        self.refresh_playback_if_playing();
    }

    /// Clip inspector in the arrange toolbar: gain, fades and color. Every
    /// change is recorded for undo. Processing lives in the right-click
    /// audio-effects window.
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

        ui.label("Clip:");
        ui.label(name);
        ui.weak("(right-click for effects)");
        ui.separator();

        // Gain (stored linearly, edited in dB).
        ui.label("Gain:");
        let mut gain_db = 20.0 * before.gain.max(1e-6).log10();
        let gain_response = ui.add(
            egui::Slider::new(&mut gain_db, -48.0..=12.0)
                .suffix(" dB")
                .max_decimals(1),
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

        if after != before {
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
            // Discrete changes (curve/color) should be heard immediately; while
            // a slider is being dragged we wait for release to avoid glitching.
            let using_pointer = ui.ctx().is_using_pointer();
            if !using_pointer || in_changed || out_changed {
                self.refresh_playback_if_playing();
            } else {
                self.needs_audio_refresh = true;
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
                        for (index, description) in &past {
                            if ui
                                .selectable_label(*index == current, format!("{}. {}", index, description))
                                .clicked()
                            {
                                jump = Some(*index);
                            }
                        }
                        for (index, description) in &future {
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
                    });
            });

        self.history_open = open;
        if let Some(target) = jump {
            if target != self.history.past_len() {
                self.jump_history_to(target);
            }
        }
    }

    /// Floating audio-effects editor, opened by right-clicking a clip.
    fn show_effects_window(&mut self, ctx: &egui::Context) {
        let Some(mut window) = self.effects_window.clone() else {
            return;
        };
        let clip = self.project.audio_clips.get(window.clip_id);
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
        let has_gain_fades = clip.map_or(false, |clip| {
            (clip.gain - 1.0).abs() > 1e-4
                || clip.fade_in.is_some()
                || clip.fade_out.is_some()
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

        egui::Window::new("Audio Effects")
            .id(egui::Id::new("audio_effects_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(430.0)
            .max_width(460.0)
            .show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.horizontal(|ui| {
                    ui.label("Clip:");
                    ui.label(egui::RichText::new(clip_name).strong());
                });
                if let Some(seconds) = clip_seconds {
                    ui.weak(format!("Length: {:.2} s", seconds));
                }
                ui.separator();

                if clip.is_none() {
                    ui.label("This clip is no longer in the project.");
                    return;
                }

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(120.0);
                        ui.label(
                            egui::RichText::new("AVAILABLE EFFECTS")
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
                        for effect in EffectKind::ALL {
                            if ui
                                .selectable_label(window.selected == effect, effect.label())
                                .clicked()
                            {
                                window.selected = effect;
                            }
                        }
                    });

                    ui.separator();

                    ui.vertical(|ui| {
                        ui.set_min_width(230.0);
                        match window.selected {
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
                        }

                        ui.add_space(10.0);
                        let can_apply = match window.selected {
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
                                consolidation_count >= 2 && !consolidation_locked
                            }
                        };
                        if ui
                            .add_enabled(can_apply, egui::Button::new("Apply"))
                            .clicked()
                        {
                            apply = true;
                        }
                    });
                });
            });

        if !open {
            self.effects_window = None;
            return;
        }

        if apply {
            match window.selected {
                EffectKind::Normalize => self.normalize_clip(window.clip_id),
                EffectKind::LufsNormalize => {
                    self.lufs_normalize_clip(window.clip_id, window.lufs_target);
                }
                EffectKind::BakeIntoClip => {
                    self.bake_into_clip(window.clip_id);
                }
                EffectKind::Reverse => self.reverse_clip(window.clip_id),
                EffectKind::PitchShift => {
                    if self.apply_pitch_stretch(
                        window.clip_id,
                        window.pitch,
                        1.0,
                        window.formants,
                        TimeStretchMode::Elastic,
                    ) {
                        window.pitch = 0.0;
                    }
                }
                EffectKind::TimeStretch => {
                    if self.apply_pitch_stretch(
                        window.clip_id,
                        0.0,
                        window.stretch,
                        false,
                        window.stretch_mode,
                    ) {
                        window.stretch = 1.0;
                    }
                }
                EffectKind::BitCrusher => {
                    let bits = window.crush_bits.round() as u32;
                    let downsample = window.crush_downsample.round() as u32;
                    self.render_clip_effect(
                        window.clip_id,
                        move |samples, channels, _sample_rate| {
                            bit_crush(samples, channels, bits, downsample);
                        },
                    );
                }
                EffectKind::Saturation => {
                    let drive_db = window.drive_db;
                    let mix = window.saturation_mix / 100.0;
                    self.render_clip_effect(
                        window.clip_id,
                        move |samples, _channels, _sample_rate| {
                            saturate(samples, drive_db, mix);
                        },
                    );
                }
                EffectKind::Filter => {
                    let kind = window.filter_kind;
                    let cutoff = window.filter_cutoff;
                    self.render_clip_effect(
                        window.clip_id,
                        move |samples, channels, sample_rate| {
                            filter_audio(samples, channels, sample_rate, cutoff, kind);
                        },
                    );
                }
                EffectKind::Tremolo => {
                    let rate = window.tremolo_rate;
                    let depth = window.tremolo_depth / 100.0;
                    self.render_clip_effect(
                        window.clip_id,
                        move |samples, channels, sample_rate| {
                            tremolo(samples, channels, sample_rate, rate, depth);
                        },
                    );
                }
                EffectKind::Consolidate => {
                    let previous = window.clip_id;
                    self.consolidate_clip(previous);
                    if self.selected_clip != Some(previous) {
                        // The clip was replaced by the consolidated result.
                        self.effects_window = None;
                        return;
                    }
                }
            }
        }

        self.effects_window = Some(window);
    }
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
    } else if response.dragged() {
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.1 } else { 1.0 };
        norm -= response.drag_delta().y / KNOB_DRAG_PIXELS * speed;
        norm = norm.clamp(0.0, 1.0);
    }

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
            if self.needs_audio_refresh {
                self.needs_audio_refresh = false;
                self.refresh_playback_if_playing();
            }
        }

        // Handle keyboard shortcuts
        self.handle_keyboard(ctx);

        // Pick up finished waveform overviews
        self.poll_waveforms();

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

        // Top transport bar
        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸" } else { "▶" }).clicked() {
                    self.toggle_playback();
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

                ui.separator();
                ui.label(format!("Project: {}", self.project.name));
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
                    }
                }
                ui.separator();
                ui.label(format!("Tracks: {}", self.project.track_order.len()));
            });
        });

        // Left track list
        egui::SidePanel::left("track_list").show(ctx, |ui| {
            ui.heading("Tracks");
            ui.separator();
            let mut track_changes: Vec<(TrackId, TrackShape, TrackShape, bool)> = Vec::new();
            for track_id in &self.project.track_order {
                if let Some(track) = self.project.tracks.get(track_id) {
                    let before = TrackShape::of(track);
                    let mut after = before;
                    ui.horizontal(|ui| {
                        let color = egui::Color32::from_rgb(track.color.0, track.color.1, track.color.2);
                        ui.colored_label(color, "■");
                        ui.label(&track.name);
                        ui.separator();

                        // Mute/Solo/Record Arm buttons
                        let mute_text = "M";
                        let solo_text = "S";
                        let arm_text = if after.record_arm { "●" } else { "○" };

                        let mute_color = if after.mute {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::GRAY
                        };
                        let solo_color = if after.solo {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::GRAY
                        };
                        let arm_color = if after.record_arm {
                            egui::Color32::RED
                        } else {
                            egui::Color32::GRAY
                        };

                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(mute_text).color(mute_color))
                                    .min_size([20.0, 20.0].into()),
                            )
                            .clicked()
                        {
                            after.mute = !after.mute;
                        }
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(solo_text).color(solo_color))
                                    .min_size([20.0, 20.0].into()),
                            )
                            .clicked()
                        {
                            after.solo = !after.solo;
                        }
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(arm_text).color(arm_color))
                                    .min_size([20.0, 20.0].into()),
                            )
                            .clicked()
                        {
                            after.record_arm = !after.record_arm;
                        }

                        ui.add_sized(
                            [80.0, 18.0],
                            egui::Slider::new(&mut after.volume, 0.0..=1.5).show_value(false),
                        )
                        .on_hover_text(format!("Track volume {:.0}%", after.volume * 100.0));
                        ui.label(format!("{:.0}%", after.volume * 100.0));
                    });
                    if after != before {
                        let volume_only = after.mute == before.mute
                            && after.solo == before.solo
                            && after.record_arm == before.record_arm;
                        track_changes.push((*track_id, before, after, volume_only));
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
                    // Defer the mixer reload until the pointer is released.
                    self.needs_audio_refresh = true;
                } else {
                    self.refresh_playback_if_playing();
                }
            }

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
                ui.separator();

                // Snap grid for moving/trimming clips (hold Alt to bypass).
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
            if self.selected_clip.is_some() {
                ui.horizontal_wrapped(|ui| {
                    self.clip_properties_ui(ui);
                });
            }
            ui.separator();

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
            let track_height = 80.0;
            let ruler_height = 30.0;

            // ---- Ruler ----
            let ruler_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + ruler_height),
            );
            painter.rect_filled(ruler_rect, 0.0, egui::Color32::from_gray(25));

            // ---- Loop region ----
            if self.loop_enabled && self.loop_end_secs > self.loop_start_secs {
                let loop_start = view.x(rect, self.loop_start_secs);
                let loop_end = view.x(rect, self.loop_end_secs);
                let band = egui::Rect::from_min_max(
                    egui::pos2(loop_start, rect.top()),
                    egui::pos2(loop_end, rect.top() + ruler_height),
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

            // ---- Ruler seek: click or drag the ruler to place the playhead ----
            // Applies the current snap grid, so with Snap set to Bar/Beat a
            // click lands exactly on a bar/beat (Alt bypasses snapping).
            let ruler_response = ui.interact(
                ruler_rect,
                ui.id().with("ruler_seek"),
                egui::Sense::click_and_drag(),
            );
            let playhead_x_before = view.x(rect, self.playhead_secs);
            let pointer_on_playhead = pointer_pos
                .map(|p| (p.x - playhead_x_before).abs() <= 8.0)
                .unwrap_or(false);
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            let seek_pos = if ruler_response.dragged() {
                ruler_response.interact_pointer_pos()
            } else if ruler_response.clicked() && !pointer_on_playhead {
                ruler_response.interact_pointer_pos()
            } else if pressed && ruler_response.hovered() && !pointer_on_playhead {
                pointer_pos
            } else {
                None
            };
            if let Some(pos) = seek_pos {
                // Seeking stops the transport (and ends an active take).
                if self.playing || self.recording {
                    self.pause_transport();
                }
                // Allow placing the playhead anywhere on screen, including
                // past the last clip.
                let max_t = self.timeline_duration().max(view.t(rect, rect.right()));
                let t = view.t(rect, pos.x).clamp(0.0, max_t);
                self.playhead_secs = self.snap_secs(t);
            }
            if ruler_response.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }

            // ---- Track lanes and clips ----
            let track_layout: Vec<(egui::Color32, Vec<ClipId>)> = self
                .project
                .track_order
                .iter()
                .filter_map(|id| {
                    self.project.tracks.get(id).map(|track| {
                        (
                            egui::Color32::from_rgb(track.color.0, track.color.1, track.color.2),
                            track.clips.clone(),
                        )
                    })
                })
                .collect();

            let track_y_start = rect.top() + ruler_height;
            let mut clip_rects: Vec<(ClipId, egui::Rect)> = Vec::new();

            for (track_idx, (color, clips)) in track_layout.iter().enumerate() {
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

                for clip_id in clips {
                    if let Some(clip) = self.project.audio_clips.get(*clip_id) {
                        let clip_rect = draw_audio_clip(
                            &painter,
                            &mut self.detail_cache,
                            clip,
                            track_rect,
                            *color,
                            view,
                            project_sr,
                            self.selected_clip == Some(*clip_id),
                        );
                        if clip_rect != egui::Rect::NOTHING {
                            clip_rects.push((*clip_id, clip_rect));
                        }
                    }
                }

                // ---- Automatic crossfade overlay ----
                // The mixer equal-power crossfades overlapping unmuted clips
                // on the same track; show where that happens.
                let mut spans: Vec<(f64, f64)> = Vec::new();
                for clip_id in clips {
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
            if primary_clicked && hovered {
                if let Some(pos) = pointer_pos {
                    let clicked = clip_rects
                        .iter()
                        .rev()
                        .find(|(_, r)| r.contains(pos))
                        .map(|(id, _)| *id);
                    if clicked.is_some() {
                        self.selected_clip = clicked;
                    } else if pos.y > rect.top() + ruler_height {
                        self.selected_clip = None;
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
            if edit_response.drag_started() {
                // Classify the gesture from where the pointer went down, not
                // from where it is after egui's drag threshold: otherwise edge
                // grabs would already be inside the body and become moves.
                if let Some(pos) = press_origin.or(pointer_pos) {
                    self.begin_clip_drag(pos, &clip_rects, rect, view);
                }
            }
            if edit_response.dragged() {
                if let Some(pos) = pointer_pos {
                    self.update_clip_drag(pos, rect, track_y_start, track_height, view);
                }
            }
            if edit_response.drag_stopped() {
                self.finish_clip_drag();
            }

            // ---- Right-click opens the floating audio-effects editor ----
            if edit_response.secondary_clicked() {
                if let Some(pos) = pointer_pos {
                    if let Some((clip_id, _)) = Self::clip_hit(&clip_rects, pos) {
                        self.selected_clip = Some(clip_id);
                        self.effects_window = Some(EffectsWindow::new(clip_id));
                    }
                }
            }

            // Hover feedback: resize cursor over trim/fade zones, grab over bodies.
            if self.drag.is_none() && !self.dragging_playhead && hovered {
                if let Some(pos) = pointer_pos {
                    if let Some((_, mode)) = Self::clip_hit(&clip_rects, pos) {
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
            if response.dragged() {
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
            } else if response.drag_stopped() {
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
                let sel = self
                    .selected_clip
                    .and_then(|id| self.project.audio_clips.get(id))
                    .and_then(|clip| clip.source.path.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("none");
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

        self.show_effects_window(ctx);
        self.show_history_window(ctx);

        // Request repaint for smooth playback and async waveform polling
        ctx.request_repaint();
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
            // Undo / redo
            if ctx.input(|i| i.key_pressed(Key::Z)) {
                if modifiers.shift {
                    self.redo();
                } else {
                    self.undo();
                }
            }
            if ctx.input(|i| i.key_pressed(Key::Y)) {
                self.redo();
            }
            // Clipboard and duplicate
            if ctx.input(|i| i.key_pressed(Key::C)) {
                self.copy_selected_clip();
            }
            if ctx.input(|i| i.key_pressed(Key::V)) {
                self.paste_clip();
            }
            if ctx.input(|i| i.key_pressed(Key::D)) {
                self.duplicate_selected_clip();
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

        // R: Record (toggle recording)
        if ctx.input(|i| i.key_pressed(Key::R)) {
            self.toggle_recording();
        }

        // S: split the selected clip at the playhead
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.split_selected_clip();
        }

        // Delete/Backspace: remove the selected clip (recoverable via undo)
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
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

            // An armed take starts capturing the moment the transport rolls.
            if let Some(track_id) = self.pending_record_track.take() {
                self.begin_recording(track_id);
            }
        }
    }

    /// Stop the transport. If a take is in progress it is ended here, so the
    /// recorded waveform appears as soon as the transport is paused instead of
    /// only when the record button is toggled off.
    fn pause_transport(&mut self) {
        self.playing = false;
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
            // Rebuild the mixer from the playhead so the new range takes
            // effect immediately.
            self.start_playback();
        }
    }

    /// Load the project's audio clips into the engine and start sound output
    /// from the current playhead position.
    fn start_playback(&mut self) {
        let out_rate = self.engine.output_sample_rate().max(1);
        let clips = self.collect_playable_clips(out_rate);
        let start_frame = (self.playhead_secs.max(0.0) * out_rate as f64) as u64;
        self.sync_playback_loop();
        self.engine.set_playback_clips(clips, start_frame);
        if let Err(e) = self.engine.start() {
            self.engine.clear_playback();
            self.playing = false;
            self.error_message = Some(format!("Failed to start playback: {}", e));
        }
    }

    /// Read the audio clips that are placed on tracks into playable buffers.
    /// Timeline positions are converted from project samples to output-device
    /// frames, and each buffer is sliced to the clip's trimmed source range.
    fn collect_playable_clips(&self, out_rate: u32) -> Vec<PlayableClip> {
        let project_sr = self.project.sample_rate.max(1) as f64;
        let any_solo = self.project.tracks.values().any(|track| track.solo);
        let mut playable = Vec::new();

        for (track_index, track_id) in self.project.track_order.iter().enumerate() {
            let Some(track) = self.project.tracks.get(track_id) else {
                continue;
            };
            let Some(track_gain) = audible_track_gain(track, any_solo) else {
                continue;
            };
            for clip_id in &track.clips {
                let Some(clip) = self.project.audio_clips.get(*clip_id) else {
                    continue;
                };
                if clip.muted || clip.length == 0 {
                    continue;
                }
                match load_wav_interleaved(&clip.source.path) {
                    Ok((samples, channels, sample_rate)) => {
                        let channel_count = channels.max(1) as usize;
                        let start = (clip.source_offset as usize)
                            .saturating_mul(channel_count)
                            .min(samples.len());
                        let end = start
                            .saturating_add(
                                (clip.length as usize).saturating_mul(channel_count),
                            )
                            .min(samples.len());
                        if end <= start {
                            continue;
                        }

                        let start_frame = (clip.timeline_position.sample as f64
                            * out_rate as f64
                            / project_sr)
                            .round() as u64;
                        playable.push(PlayableClip {
                            samples: samples[start..end].to_vec(),
                            channels,
                            sample_rate,
                            start_frame,
                            track: track_index as u32,
                            gain: clip.gain * track_gain,
                            fade_in: clip.fade_in,
                            fade_out: clip.fade_out,
                        });
                    }
                    Err(e) => eprintln!(
                        "Playback: skipping {}: {}",
                        clip.source.path.display(),
                        e
                    ),
                }
            }
        }

        playable
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
                // Transport already rolling: punch in immediately.
                self.begin_recording(track_id);
            } else {
                // Wait for the transport to roll before capturing audio, so the
                // take starts exactly where the playhead starts moving.
                self.pending_record_track = Some(track_id);
                self.error_message = None;
            }
        }
    }

    /// Open the input device and start capturing a take at the current playhead.
    fn begin_recording(&mut self, track_id: TrackId) {
        // Unique file per take so new recordings never overwrite older ones.
        let file_path = std::path::PathBuf::from(format!(
            "recording_{}.wav",
            uuid::Uuid::new_v4()
        ));
        if let Err(e) = self.engine.start_recording(track_id, file_path) {
            self.recording = false;
            self.pending_record_track = None;
            self.error_message = Some(format!("Recording failed: {}", e));
        } else {
            self.recording = true;
            self.record_start_secs = Some(self.playhead_secs);
            self.error_message = None;
        }
    }

    /// Stop an active recording and add the finished file to the timeline as an
    /// audio clip (which also queues waveform generation).
    fn stop_recording_and_create_clip(&mut self) {
        self.recording = false;
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

        let clip_id = self.project.audio_clips.insert(clip);
        if let Some(track) = self.project.tracks.get_mut(&track_id) {
            track.add_clip(clip_id);
        }

        self.selected_clip = Some(clip_id);

        self.spawn_overview(clip_id, file_path);
    }
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

    let full_rect = egui::Rect::from_min_max(
        egui::pos2(x_start, lane.top() + 18.0),
        egui::pos2(x_start + width, lane.bottom() - 6.0),
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
    let bg_alpha = if selected { 90 } else { 55 };
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

    // ---- Labels ----
    let name = clip
        .source
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("clip");
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
        audible_track_gain, bit_crush, clip_gain_at, contiguous_clip_span, filter_audio,
        knob_from_normalized, knob_to_normalized, load_wav_interleaved,
        load_wav_range_interleaved, loop_range_frames, measure_integrated_loudness,
        merge_continuous_edit, processed_file_stem, render_pitch_stretch, reverse_interleaved,
        saturate, tape_stretch_interleaved, tremolo, wav_file_info, wav_peak, write_wav_f32,
        write_wav_range, write_wav_range_with_envelope, ClipShape, ClipSourceState, EditAction,
        FilterKind, TrackShape,
    };
    use daw_core::audio_pool::AudioFileRef;
    use daw_core::clip::{AudioClip, ClipId, Fade, FadeCurve, TimeStretchMode, TimelinePos};
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
    fn track_state_descriptions_name_the_toggle() {
        let track_id = daw_core::track::TrackId::default();
        let base = TrackShape {
            mute: false,
            solo: false,
            record_arm: false,
            volume: 1.0,
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
}
