use crate::dsp::{peak_and_rms, AudioBuffer};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Instant;

/// Rate-limited error logger to prevent spam
struct RateLimitedLogger {
    last_log: Mutex<Option<Instant>>,
    min_interval: std::time::Duration,
}

impl RateLimitedLogger {
    fn new(min_interval_ms: u64) -> Self {
        Self {
            last_log: Mutex::new(None),
            min_interval: std::time::Duration::from_millis(min_interval_ms),
        }
    }

    fn log(&self, message: &str) {
        let mut last = self.last_log.lock().unwrap();
        let now = Instant::now();
        if last.map_or(true, |t| now.duration_since(t) >= self.min_interval) {
            eprintln!("{}", message);
            *last = Some(now);
        }
    }
}

/// Wrapper to make cpal::Stream Send + Sync safe
/// cpal::Stream contains a *mut () internally which is not Send/Sync
/// We wrap it in a struct that manually implements Send + Sync
/// Safety: The stream is only accessed from the audio thread, so it's safe to mark as Send
struct SendStream(Option<Stream>);

unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

impl SendStream {
    fn new(stream: Stream) -> Self {
        SendStream(Some(stream))
    }

    fn take(&mut self) -> Option<Stream> {
        self.0.take()
    }
}

/// Wrapper to make `cpal::Device` Send + Sync safe.
///
/// ALSA device handles are not `Send`, which prevents caching the devices we
/// hand to the UI. We only ever touch a cached device while holding the backend
/// mutex on the thread that enumerated it, mirroring CPAL's own handling of
/// stream handles.
struct SendDevice(cpal::Device);

unsafe impl Send for SendDevice {}
unsafe impl Sync for SendDevice {}

/// Rank sample formats by how much we prefer to record with them.
fn sample_format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::U16 => 3,
        SampleFormat::U32 => 4,
        SampleFormat::F64 => 5,
        _ => 6,
    }
}

/// Rank channel counts: prefer stereo, then mono, then anything else.
fn channel_rank(channels: u16) -> u8 {
    match channels {
        2 => 0,
        1 => 1,
        _ => 2,
    }
}

/// Choose a sensible capture configuration for `device`.
///
/// We deliberately avoid blindly taking the highest channel count: ALSA's
/// `plug`/`sysdefault` PCMs advertise up to 32 channels and exotic formats
/// (e.g. f64), which usually fail to open. Instead we prefer, in order:
/// a supported sample format (f32 first), stereo, and the requested rate.
fn select_input_config(
    device: &cpal::Device,
    desired_rate: u32,
) -> Result<cpal::SupportedStreamConfig> {
    let configs: Vec<_> = device.supported_input_configs()?.collect();
    if configs.is_empty() {
        return device
            .default_input_config()
            .map_err(|e| anyhow::anyhow!("device has no usable input configuration: {}", e));
    }

    let rate_ok = |c: &cpal::SupportedStreamConfigRange| {
        c.min_sample_rate().0 <= desired_rate && c.max_sample_rate().0 >= desired_rate
    };

    if let Some(c) = configs
        .iter()
        .filter(|c| rate_ok(c))
        .min_by_key(|c| (sample_format_rank(c.sample_format()), channel_rank(c.channels())))
        .cloned()
    {
        return Ok(c.with_sample_rate(cpal::SampleRate(desired_rate)));
    }

    // No configuration supports the desired rate; fall back to the best format
    // and channel layout at the device's minimum rate.
    let c = configs
        .iter()
        .min_by_key(|c| (sample_format_rank(c.sample_format()), channel_rank(c.channels())))
        .cloned()
        .expect("configs is non-empty");
    let rate = c.min_sample_rate().0.max(1);
    Ok(c.with_sample_rate(cpal::SampleRate(rate)))
}

/// Turn a raw ALSA PCM name into something friendlier for display.
///
/// `sysdefault:CARD=Microphone` -> `Microphone`
/// `sysdefault`                 -> `System Default`
/// `hw:CARD=Generic_1,DEV=0`    -> `Generic 1`
fn clean_device_name(name: &str) -> String {
    let name = name.trim();
    let (pcm_type, rest) = match name.split_once(':') {
        Some((p, r)) => (p, r),
        None => ("", name),
    };

    // Prefer the human-readable card name, when one is present.
    let card = regex::Regex::new(r"CARD=([^,]+)")
        .unwrap()
        .captures(rest)
        .map(|caps| caps[1].replace('_', " ").trim().to_string())
        .filter(|c| !c.is_empty());

    // Prefix that distinguishes otherwise identical PCMs of the same card.
    let qualifier = match pcm_type {
        "front" => Some("Front"),
        "surround40" => Some("Surround 4.0"),
        "surround51" => Some("Surround 5.1"),
        "surround71" => Some("Surround 7.1"),
        "rear" => Some("Rear"),
        "center_lfe" => Some("Center/LFE"),
        "dmix" => Some("DMix"),
        "dsnoop" => Some("DSnoop"),
        "hw" => Some("Hardware"),
        _ => None,
    };

    let base = if let Some(card) = card {
        card
    } else {
        let lower = name.to_lowercase();
        match lower.as_str() {
            "sysdefault" => "System Default".to_string(),
            "default" => "Default".to_string(),
            "" => "Audio Device".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => "Audio Device".to_string(),
                }
            }
        }
    };

    match qualifier {
        Some(q) => format!("{} ({})", q, base),
        None => base,
    }
}

/// Whether an ALSA PCM name routes through the sound server (PipeWire/Pulse)
/// and can therefore be shared with the rest of the desktop.
///
/// Raw PCMs (`hw:`, `front:`, `sysdefault:CARD=…`, `surround…`) open the
/// hardware exclusively: grabbing one mutes the whole system and blocks
/// PipeWire. We only ever offer the shared routing PCMs.
fn is_shared_pcm(raw_name: &str) -> bool {
    let base = raw_name.split(':').next().unwrap_or(raw_name);
    matches!(base, "default" | "pipewire" | "pulse")
}

/// Ranking for choosing the most desirable shared PCM as the default.
fn shared_pcm_rank(raw_name: &str) -> Option<u8> {
    match raw_name.split(':').next().unwrap_or(raw_name) {
        "pipewire" => Some(0),
        "pulse" => Some(1),
        "default" => Some(2),
        _ => None,
    }
}

/// Build an [`AudioDeviceInfo`] for a device, or `None` if it cannot capture.
///
/// Shared PCMs (`pipewire`/`pulse`/`default`) are always kept — they negotiate
/// format at stream time — while raw hardware PCMs are only kept when they
/// advertise usable input configurations.
fn describe_input_device(
    device: &cpal::Device,
    index: usize,
    raw_name: String,
) -> Option<AudioDeviceInfo> {
    let shared = is_shared_pcm(&raw_name);

    let mut sample_rates = Vec::new();
    let mut max_input_channels = 0u16;
    if let Ok(configs) = device.supported_input_configs() {
        for config in configs {
            sample_rates.push(config.min_sample_rate().0);
            sample_rates.push(config.max_sample_rate().0);
            max_input_channels = max_input_channels.max(config.channels());
        }
    }
    sample_rates.sort_unstable();
    sample_rates.dedup();

    // A raw PCM with no usable input config cannot capture; drop it.
    if !shared && max_input_channels == 0 {
        return None;
    }
    if shared && max_input_channels == 0 {
        max_input_channels = 2;
        sample_rates = vec![44_100, 48_000];
    }

    let name = clean_device_name(&raw_name);

    Some(AudioDeviceInfo {
        id: format!("input_{}_{}", index, name.replace(' ', "_")),
        name,
        raw_name,
        is_default_input: false,
        is_default_output: false,
        max_input_channels: max_input_channels as u32,
        max_output_channels: 0,
        sample_rates,
    })
}

pub trait AudioBackend: Send + Sync {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn sample_rate(&self) -> f32;
    fn channels(&self) -> u16;
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    /// Start capturing from `device`. Returns the actual `(sample_rate, channels)`
    /// of the opened stream so the caller can match its writer to it.
    fn start_input_stream(&self, device: &AudioDeviceInfo, desired_sample_rate: u32, sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>) -> Result<(u32, u16)>;
    fn stop_input_stream(&self) -> Result<()>;
    /// Set (or clear) the mixer that the output stream pulls samples from.
    fn set_playback_mixer(&self, mixer: Option<crate::playback::PlaybackMixer>);
    /// Update the loop range on the current playback mixer, if any.
    fn set_playback_loop(&self, _range: Option<(u64, u64)>) {}
    /// Update the metronome on the current playback mixer, if any.
    fn set_playback_metronome(&self, _config: Option<crate::playback::MetronomeConfig>) {}
    /// Peak and RMS of the most recent input block, if the input stream is
    /// running.
    fn input_level(&self) -> Option<(f32, f32)> {
        None
    }
    /// Current playback position in output frames, if playback is loaded.
    fn playback_position(&self) -> Option<u64>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub raw_name: String,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub max_input_channels: u32,
    pub max_output_channels: u32,
    pub sample_rates: Vec<u32>,
}

/// Most recent input block levels, shared with the UI meter.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputLevel {
    pub peak: f32,
    pub rms: f32,
}

pub struct CpalBackend {
    sample_rate: f32,
    channels: u16,
    running: Arc<Mutex<bool>>,
    input_running: Arc<Mutex<bool>>,
    input_sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>,
    input_level: Arc<Mutex<InputLevel>>,
    output_stream: Arc<Mutex<SendStream>>,
    input_stream: Arc<Mutex<SendStream>>,
    playback: Arc<Mutex<Option<crate::playback::PlaybackMixer>>>,
    input_device_cache: Arc<Mutex<Vec<(String, SendDevice)>>>,
    error_logger: Arc<RateLimitedLogger>,
}

impl CpalBackend {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels();

        Ok(Self {
            sample_rate,
            channels,
            running: Arc::new(Mutex::new(false)),
            input_running: Arc::new(Mutex::new(false)),
            input_sender: Arc::new(Mutex::new(None)),
            input_level: Arc::new(Mutex::new(InputLevel::default())),
            output_stream: Arc::new(Mutex::new(SendStream(None))),
            input_stream: Arc::new(Mutex::new(SendStream(None))),
            playback: Arc::new(Mutex::new(None)),
            input_device_cache: Arc::new(Mutex::new(Vec::new())),
            error_logger: Arc::new(RateLimitedLogger::new(1000)),
        })
    }

pub fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let default_input_name = host.default_input_device().and_then(|d| d.name().ok());

        // Gather names first (cheap, no device opening) so we can decide which
        // devices to keep without ever opening raw hardware PCMs.
        let named: Vec<(usize, cpal::Device, String)> = host
            .input_devices()?
            .enumerate()
            .map(|(index, device)| {
                let raw_name = device
                    .name()
                    .unwrap_or_else(|_| format!("Unknown Input {}", index));
                (index, device, raw_name)
            })
            .collect();

        // Only expose sound-server PCMs so recording never steals the card from
        // the rest of the desktop. If none exist (bare ALSA), fall back to
        // whatever we found.
        let has_shared = named.iter().any(|(_, _, raw)| is_shared_pcm(raw));

        let mut devices: Vec<AudioDeviceInfo> = Vec::new();
        let mut cache: Vec<(String, SendDevice)> = Vec::new();
        for (index, device, raw_name) in named {
            if has_shared && !is_shared_pcm(&raw_name) {
                continue;
            }
            if let Some(info) = describe_input_device(&device, index, raw_name) {
                cache.push((info.raw_name.clone(), SendDevice(device)));
                devices.push(info);
            }
        }

        // Flag exactly one device as the default input, preferring the most
        // direct shared PCM (`pipewire` > `pulse` > `default`), then fall back
        // to the host's default input, then the first entry.
        let best_rank = devices
            .iter()
            .filter_map(|d| shared_pcm_rank(&d.raw_name))
            .min();
        let default_index = best_rank
            .and_then(|rank| {
                devices
                    .iter()
                    .position(|d| shared_pcm_rank(&d.raw_name) == Some(rank))
            })
            .or_else(|| {
                default_input_name
                    .as_deref()
                    .and_then(|raw| devices.iter().position(|d| d.raw_name == raw))
            })
            .or(if devices.is_empty() { None } else { Some(0) });
        if let Some(idx) = default_index {
            devices[idx].is_default_input = true;
        }

        *self
            .input_device_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = cache;

        Ok(devices)
    }

    pub fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        let default_output = host.default_output_device();
        let default_output_name = default_output.as_ref().and_then(|d| d.name().ok());

        let all: Vec<(usize, cpal::Device, String)> = host
            .output_devices()?
            .enumerate()
            .map(|(index, device)| {
                let raw_name = device
                    .name()
                    .unwrap_or_else(|_| format!("Unknown Output {}", index));
                (index, device, raw_name)
            })
            .collect();

        // Same policy as inputs: only offer shared sound-server PCMs so we never
        // take exclusive ownership of an output card.
        let has_shared = all.iter().any(|(_, _, raw)| is_shared_pcm(raw));
        let devices_iter = all
            .into_iter()
            .filter(move |(_, _, raw)| !has_shared || is_shared_pcm(raw));

        for (index, device, raw_name) in devices_iter {
            let name = clean_device_name(&raw_name);
            let is_default = default_output_name.as_ref().map(|d| d == &raw_name).unwrap_or(false);

            let mut sample_rates = Vec::new();
            let mut max_output_channels = 0;
            if let Ok(configs) = device.supported_output_configs() {
                for config in configs {
                    sample_rates.push(config.min_sample_rate().0);
                    sample_rates.push(config.max_sample_rate().0);
                    max_output_channels = max_output_channels.max(config.channels());
                }
            }
            sample_rates.sort();
            sample_rates.dedup();

            let id = format!("output_{}_{}", index, name.replace(' ', "_"));

            devices.push(AudioDeviceInfo {
                id,
                name,
                raw_name: raw_name.clone(),
                is_default_input: false,
                is_default_output: is_default,
                max_input_channels: 0,
                max_output_channels: max_output_channels as u32,
                sample_rates,
            });
        }

        Ok(devices)
    }
}

impl AudioBackend for CpalBackend {
    fn start(&self) -> Result<()> {
        let mut running_lock = self.running.lock().unwrap();
        if *running_lock {
            return Ok(());
        }
        drop(running_lock);

        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;

        let config = device.default_output_config()?;
        let _sample_rate = config.sample_rate().0 as f32;
        let _channels = config.channels();

        let running = Arc::clone(&self.running);
        let error_logger = Arc::clone(&self.error_logger);
        let playback = Arc::clone(&self.playback);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), &running, &playback, &error_logger)?,
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), &running, &playback, &error_logger)?,
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), &running, &playback, &error_logger)?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        stream.play()?;
        *running.lock().unwrap() = true;

        // Store the stream so we can stop it later
        *self.output_stream.lock().unwrap() = SendStream::new(stream);

        Ok(())
    }

    fn stop(&self) -> Result<()> {
        // Release the running flag before dropping the stream; dropping joins the
        // audio thread whose callback locks this same flag.
        {
            let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
            *running = false;
        }

        if let Some(stream) = self.output_stream.lock().unwrap_or_else(|e| e.into_inner()).take() {
            drop(stream);
        }
        Ok(())
    }

    fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.list_input_devices()
    }

    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.list_output_devices()
    }

    fn start_input_stream(&self, device_info: &AudioDeviceInfo, desired_sample_rate: u32, sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>) -> Result<(u32, u16)> {
        {
            let input_running = self.input_running.lock().unwrap_or_else(|e| e.into_inner());
            if *input_running {
                return Err(anyhow::anyhow!("Input stream is already running"));
            }
        }

        // Find the matching cpal device from the cache populated by
        // `list_input_devices`. Re-enumerating here is unreliable because ALSA
        // does not return a stable device set between calls.
        let device = {
            let cache = self
                .input_device_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache
                .iter()
                .find(|(raw, _)| raw == &device_info.raw_name)
                .or_else(|| {
                    cache
                        .iter()
                        .find(|(raw, _)| clean_device_name(raw) == device_info.name)
                })
                .map(|(_, device)| device.0.clone())
        };

        let device = device.ok_or_else(|| {
            anyhow::anyhow!(
                "Input device not found: {} (raw_name: {})",
                device_info.name,
                device_info.raw_name
            )
        })?;

        let supported_config = select_input_config(&device, desired_sample_rate)?;
        let stream_config: StreamConfig = supported_config.clone().into();
        let actual_rate = supported_config.sample_rate().0;
        let actual_channels = supported_config.channels();

        let running = Arc::clone(&self.input_running);
        let sender_clone = Arc::clone(&sender);
        let level = Arc::clone(&self.input_level);
        let error_logger = Arc::clone(&self.error_logger);

        let stream = match supported_config.sample_format() {
            SampleFormat::F32 => build_input_stream::<f32, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| s)?,
            SampleFormat::I16 => build_input_stream::<i16, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| s as f32 / 32768.0)?,
            SampleFormat::I32 => build_input_stream::<i32, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| s as f32 / 2147483648.0)?,
            SampleFormat::U16 => build_input_stream::<u16, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| (s as f32 - 32768.0) / 32768.0)?,
            SampleFormat::U32 => build_input_stream::<u32, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| (s as f64 - 2147483648.0) as f32 / 2147483648.0)?,
            SampleFormat::F64 => build_input_stream::<f64, _>(&device, &stream_config, &running, sender_clone, &level, &error_logger, |s| s as f32)?,
            other => return Err(anyhow::anyhow!("Unsupported sample format: {:?}", other)),
        };

        stream.play()?;
        *self.input_running.lock().unwrap_or_else(|e| e.into_inner()) = true;

        // Store the stream so we can stop it later
        *self.input_stream.lock().unwrap_or_else(|e| e.into_inner()) = SendStream::new(stream);

        Ok((actual_rate, actual_channels))
    }

fn stop_input_stream(&self) -> Result<()> {
        // Release the running flag *before* dropping the stream: dropping a cpal
        // stream joins its audio thread, and the callback locks this same flag.
        {
            let mut running = self.input_running.lock().unwrap_or_else(|e| e.into_inner());
            *running = false;
        }

        if let Some(stream) = self.input_stream.lock().unwrap_or_else(|e| e.into_inner()).take() {
            drop(stream);
        }
        if let Ok(mut level) = self.input_level.lock() {
            *level = InputLevel::default();
        }
        Ok(())
    }

    fn set_playback_mixer(&self, mixer: Option<crate::playback::PlaybackMixer>) {
        *self.playback.lock().unwrap_or_else(|e| e.into_inner()) = mixer;
    }

    fn set_playback_loop(&self, range: Option<(u64, u64)>) {
        if let Some(mixer) = self
            .playback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            mixer.set_loop_range(range);
        }
    }

    fn set_playback_metronome(&self, config: Option<crate::playback::MetronomeConfig>) {
        if let Some(mixer) = self
            .playback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            mixer.set_metronome(config);
        }
    }

    fn input_level(&self) -> Option<(f32, f32)> {
        let running = *self
            .input_running
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !running {
            return None;
        }
        let level = *self
            .input_level
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Some((level.peak, level.rms))
    }

    fn playback_position(&self) -> Option<u64> {
        // try_lock: never block the UI thread on the audio callback.
        self.playback
            .try_lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|mixer| mixer.position()))
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    running: &Arc<Mutex<bool>>,
    playback: &Arc<Mutex<Option<crate::playback::PlaybackMixer>>>,
    error_logger: &Arc<RateLimitedLogger>,
) -> Result<Stream>
where
    T: Sample + SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let err_fn = {
        let logger = Arc::clone(error_logger);
        move |err: cpal::StreamError| logger.log(&format!("Audio stream error: {}", err))
    };
    let running = Arc::clone(running);
    let playback = Arc::clone(playback);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            {
                let running = running.lock().unwrap_or_else(|e| e.into_inner());
                if !*running {
                    return;
                }
            }

            let mut playback_guard = playback.lock().unwrap_or_else(|e| e.into_inner());
            match playback_guard.as_mut() {
                Some(mixer) => mixer.fill(data),
                None => {
                    for sample in data.iter_mut() {
                        *sample = T::from_sample(0.0f32);
                    }
                }
            }
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

fn build_input_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    running: &Arc<Mutex<bool>>,
    sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>,
    level: &Arc<Mutex<InputLevel>>,
    error_logger: &Arc<RateLimitedLogger>,
    convert: F,
) -> Result<Stream>
where
    T: SizedSample + Send + 'static,
    F: Fn(T) -> f32 + Send + 'static,
{
    let err_fn = {
        let logger = Arc::clone(error_logger);
        move |err: cpal::StreamError| logger.log(&format!("Audio input stream error: {}", err))
    };
    let running = Arc::clone(running);
    let sender = Arc::clone(&sender);
    let level = Arc::clone(level);
    let channels = config.channels as usize;

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let running = running.lock().unwrap();
            if *running {
                if let Ok(sender_guard) = sender.lock() {
                    if let Some(tx) = sender_guard.as_ref() {
                        let frames = data.len() / channels;
                        let mut buffer = AudioBuffer::new(channels, frames);
                        for (i, sample) in data.iter().enumerate() {
                            buffer.data[i] = convert(*sample);
                        }
                        let (peak, rms) = peak_and_rms(&buffer.data);
                        if let Ok(mut meter) = level.lock() {
                            meter.peak = peak;
                            meter.rms = rms;
                        }
                        let _ = tx.send(buffer);
                    }
                }
            }
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

pub fn create_default_backend() -> Result<Box<dyn AudioBackend>> {
    Ok(Box::new(CpalBackend::new()?))
}