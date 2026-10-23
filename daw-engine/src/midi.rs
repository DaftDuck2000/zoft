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

/// Latest activity seen by a [`MidiMonitor`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MidiActivity {
    pub last_note: Option<u8>,
    pub last_velocity: u8,
    pub notes_seen: u64,
    pub active_notes: u32,
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

/// Open a MIDI input port and track note activity from it. Dropping the
/// monitor closes the port.
pub struct MidiMonitor {
    shared: Arc<Mutex<MidiActivity>>,
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
        let connection = input.connect(
            port,
            "zoft-monitor",
            move |_timestamp, message, _| {
                if message.len() < 3 {
                    return;
                }
                let status = message[0] & 0xF0;
                let note = message[1] & 0x7F;
                let velocity = message[2] & 0x7F;
                let Ok(mut activity) = state.lock() else {
                    return;
                };
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
            },
            (),
        )
        .map_err(|e| anyhow::anyhow!("Could not open MIDI input: {}", e))?;

        Ok(Self {
            shared,
            connection: Some(connection),
        })
    }

    /// Snapshot the latest note activity.
    pub fn activity(&self) -> MidiActivity {
        self.shared
            .lock()
            .map(|activity| *activity)
            .unwrap_or_default()
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
    fn note_names_use_scientific_pitch() {
        assert_eq!(midi_note_name(60), "C4");
        assert_eq!(midi_note_name(69), "A4");
        assert_eq!(midi_note_name(61), "C#4");
        assert_eq!(midi_note_name(0), "C-1");
    }
}
