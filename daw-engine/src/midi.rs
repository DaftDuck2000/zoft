//! MIDI input device enumeration and live note monitoring.

use anyhow::Result;
use midir::MidiInput;
use std::sync::{Arc, Mutex};

/// One available MIDI input port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiInputInfo {
    pub index: usize,
    pub name: String,
}

/// One available MIDI output port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiOutputInfo {
    pub index: usize,
    pub name: String,
}

/// Latest activity seen by a [`MidiMonitor`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MidiActivity {
    pub last_note: Option<u8>,
    pub last_velocity: u8,
    pub notes_seen: u64,
    pub active_notes: u32,
}

/// One note-on/off captured from a MIDI input.
#[derive(Debug, Clone, Copy)]
pub struct MidiInputEvent {
    pub timestamp_micros: u64,
    /// Local arrival time, used to place notes relative to the start of a take.
    pub received_at: std::time::Instant,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    pub on: bool,
}

/// List the MIDI input ports exposed by the system (ALSA/JACK on Linux).
pub fn list_midi_inputs() -> Result<Vec<MidiInputInfo>> {
    let input = MidiInput::new("Zoft")
        .map_err(|e| anyhow::anyhow!("MIDI init failed: {}", e))?;
    let mut devices = Vec::new();
    for (index, port) in input.ports().iter().enumerate() {
        let name = input
            .port_name(port)
            .unwrap_or_else(|_| format!("MIDI Input {}", index + 1));
        devices.push(MidiInputInfo { index, name });
    }
    Ok(devices)
}

/// List the MIDI output ports exposed by the system (ALSA/JACK on Linux).
pub fn list_midi_outputs() -> Result<Vec<MidiOutputInfo>> {
    let output = midir::MidiOutput::new("Zoft")
        .map_err(|e| anyhow::anyhow!("MIDI init failed: {}", e))?;
    let mut devices = Vec::new();
    for (index, port) in output.ports().iter().enumerate() {
        let name = output
            .port_name(port)
            .unwrap_or_else(|_| format!("MIDI Output {}", index + 1));
        devices.push(MidiOutputInfo { index, name });
    }
    Ok(devices)
}

/// Open a MIDI output port for echoing input notes (MIDI thru). Dropping the
/// connection closes the port.
pub struct MidiThru {
    connection: Option<midir::MidiOutputConnection>,
}

impl MidiThru {
    pub fn start(port_index: usize) -> Result<Self> {
        let output = midir::MidiOutput::new("Zoft")
            .map_err(|e| anyhow::anyhow!("MIDI init failed: {}", e))?;
        let ports = output.ports();
        let port = ports.get(port_index).ok_or_else(|| {
            anyhow::anyhow!("MIDI output {} is not available", port_index + 1)
        })?;
        let connection = output
            .connect(port, "zoft-thru")
            .map_err(|e| anyhow::anyhow!("Could not open MIDI output: {}", e))?;
        Ok(Self {
            connection: Some(connection),
        })
    }

    /// Send a note-on or note-off on the given channel.
    pub fn send_note(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        on: bool,
    ) -> Result<()> {
        let status = if on { 0x90 } else { 0x80 } | (channel & 0x0F);
        let Some(connection) = self.connection.as_mut() else {
            return Ok(());
        };
        connection
            .send(&[status, note & 0x7F, velocity & 0x7F])
            .map_err(|e| anyhow::anyhow!("MIDI send failed: {}", e))
    }

    /// Silence every channel when the transport stops or panic is pressed.
    pub fn all_notes_off(&mut self) -> Result<()> {
        let Some(connection) = self.connection.as_mut() else {
            return Ok(());
        };
        for channel in 0..16u8 {
            connection
                .send(&[0xB0 | channel, 123, 0])
                .map_err(|e| anyhow::anyhow!("MIDI send failed: {}", e))?;
        }
        Ok(())
    }
}

impl Drop for MidiThru {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
    }
}

/// Update one activity snapshot from a MIDI status byte.
fn record_activity(
    activity: &mut MidiActivity,
    status: u8,
    note: u8,
    velocity: u8,
) {
    match status {
        0x90 if velocity > 0 => {
            activity.last_note = Some(note);
            activity.last_velocity = velocity;
            activity.notes_seen = activity.notes_seen.saturating_add(1);
            activity.active_notes = activity.active_notes.saturating_add(1);
        }
        0x80 | 0x90 => {
            activity.last_note = Some(note);
            activity.last_velocity = 0;
            activity.active_notes = activity.active_notes.saturating_sub(1);
        }
        _ => {}
    }
}

/// Open a MIDI input port and track note activity from it. Dropping the
/// monitor closes the port.
pub struct MidiMonitor {
    shared: Arc<Mutex<MidiActivity>>,
    channels: Arc<Mutex<[MidiActivity; 16]>>,
    events: Arc<Mutex<Vec<MidiInputEvent>>>,
    connection: Option<midir::MidiInputConnection<()>>,
}

impl MidiMonitor {
    pub fn start(port_index: usize) -> Result<Self> {
        let mut input = MidiInput::new("Zoft")
            .map_err(|e| anyhow::anyhow!("MIDI init failed: {}", e))?;
        let ports = input.ports();
        let port = ports
            .get(port_index)
            .ok_or_else(|| anyhow::anyhow!("MIDI input {} is not available", port_index + 1))?;

        let shared = Arc::new(Mutex::new(MidiActivity::default()));
        let state = Arc::clone(&shared);
        let channels = Arc::new(Mutex::new([MidiActivity::default(); 16]));
        let channel_state = Arc::clone(&channels);
        let events = Arc::new(Mutex::new(Vec::<MidiInputEvent>::new()));
        let event_queue = Arc::clone(&events);
        let connection = input.connect(
            port,
            "zoft-monitor",
            move |timestamp, message, _| {
                if message.len() < 3 {
                    return;
                }
                let status = message[0] & 0xF0;
                let channel = message[0] & 0x0F;
                let note = message[1] & 0x7F;
                let velocity = message[2] & 0x7F;
                let is_on = status == 0x90 && velocity > 0;
                let is_off = status == 0x80 || (status == 0x90 && velocity == 0);
                if is_on || is_off {
                    if let Ok(mut queue) = event_queue.lock() {
                        if queue.len() >= 8192 {
                            queue.remove(0);
                        }
                        queue.push(MidiInputEvent {
                            timestamp_micros: timestamp,
                            received_at: std::time::Instant::now(),
                            channel,
                            note,
                            velocity,
                            on: is_on,
                        });
                    }
                }
                if let Ok(mut activity) = state.lock() {
                    record_activity(&mut activity, status, note, velocity);
                }
                if let Ok(mut channels) = channel_state.lock() {
                    if let Some(activity) = channels.get_mut(channel as usize) {
                        record_activity(activity, status, note, velocity);
                    }
                }
            },
            (),
        )
        .map_err(|e| anyhow::anyhow!("Could not open MIDI input: {}", e))?;

        Ok(Self {
            shared,
            channels,
            events,
            connection: Some(connection),
        })
    }

    /// Drain captured note-on/off events (used while recording a MIDI take).
    pub fn drain_events(&self) -> Vec<MidiInputEvent> {
        self.events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }

    /// Snapshot the latest note activity.
    pub fn activity(&self) -> MidiActivity {
        self.shared
            .lock()
            .map(|activity| *activity)
            .unwrap_or_default()
    }

    /// Snapshot activity for one MIDI channel, or all channels when `None`.
    pub fn activity_for_channel(&self, channel: Option<u8>) -> MidiActivity {
        match channel {
            Some(channel) if channel < 16 => self
                .channels
                .lock()
                .map(|channels| channels[channel as usize])
                .unwrap_or_default(),
            _ => self.activity(),
        }
    }
}

impl Drop for MidiMonitor {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
    }
}

/// MIDI note number to scientific pitch notation, e.g. 60 → "C4".
pub fn midi_note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = note as i32 / 12 - 1;
    format!("{}{}", NAMES[(note % 12) as usize], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_tracks_note_on_and_off() {
        let mut activity = MidiActivity::default();
        record_activity(&mut activity, 0x90, 60, 100);
        assert_eq!(activity.last_note, Some(60));
        assert_eq!(activity.last_velocity, 100);
        assert_eq!(activity.active_notes, 1);
        record_activity(&mut activity, 0x80, 60, 0);
        assert_eq!(activity.active_notes, 0);
        assert_eq!(activity.last_velocity, 0);
    }

    #[test]
    fn note_names_use_scientific_pitch() {
        assert_eq!(midi_note_name(60), "C4");
        assert_eq!(midi_note_name(69), "A4");
        assert_eq!(midi_note_name(61), "C#4");
        assert_eq!(midi_note_name(0), "C-1");
    }
}
