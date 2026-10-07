use crate::commands::Command;
use crate::project::Project;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct NoopCommand;

impl Command for NoopCommand {
    fn execute(&mut self, _ctx: &mut crate::commands::CommandContext) -> Result<()> { Ok(()) }
    fn undo(&mut self, _ctx: &mut crate::commands::CommandContext) -> Result<()> { Ok(()) }
    fn description(&self) -> String { "Noop".to_string() }
    fn is_noop(&self) -> bool { true }
}

pub struct UndoHistory {
    past: VecDeque<HistoryEntry>,
    future: VecDeque<HistoryEntry>,
    max_size: usize,
    snapshot_interval: usize,
    command_count: usize,
    macro_recording: Option<MacroRecorder>,
}

pub struct HistoryEntry {
    pub command: Box<dyn Command>,
    pub timestamp: u64,
    pub macro_group: Option<MacroId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MacroId(pub u64);

pub struct MacroRecorder {
    commands: Vec<Box<dyn Command>>,
    is_recording: bool,
    macro_id: MacroId,
}

impl MacroRecorder {
    pub fn new(macro_id: MacroId) -> Self {
        Self {
            commands: Vec::new(),
            is_recording: false,
            macro_id,
        }
    }

    pub fn start(&mut self) {
        self.is_recording = true;
        self.commands.clear();
    }

    pub fn record(&mut self, command: Box<dyn Command>) {
        if self.is_recording {
            self.commands.push(command);
        }
    }

    pub fn finish(&mut self) -> Option<Box<dyn Command>> {
        self.is_recording = false;
        if self.commands.len() > 1 {
            Some(Box::new(crate::commands::MacroCommand {
                commands: self.commands.drain(..).collect(),
            }))
        } else {
            self.commands.pop()
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording
    }
}

impl UndoHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            past: VecDeque::with_capacity(max_size),
            future: VecDeque::with_capacity(max_size),
            max_size,
            snapshot_interval: 50,
            command_count: 0,
            macro_recording: None,
        }
    }

    pub fn push(&mut self, command: Box<dyn Command>) {
        if let Some(ref mut macro_rec) = self.macro_recording {
            if macro_rec.is_recording() {
                macro_rec.record(command);
                return;
            }
        }

        self.past.push_back(HistoryEntry {
            command,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            macro_group: self.macro_recording.as_ref().map(|m| m.macro_id),
        });

        if self.past.len() > self.max_size {
            self.past.pop_front();
        }

        self.future.clear();
        self.command_count += 1;

        if self.command_count % self.snapshot_interval == 0 {
            self.create_snapshot();
        }
    }

    pub fn undo(&mut self) -> Option<Box<dyn Command>> {
        if let Some(HistoryEntry { command, timestamp, macro_group }) = self.past.pop_back() {
            self.future.push_front(HistoryEntry { command: Box::new(NoopCommand), timestamp, macro_group });
            Some(command)
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<Box<dyn Command>> {
        if let Some(HistoryEntry { command, timestamp, macro_group }) = self.future.pop_front() {
            self.past.push_back(HistoryEntry { command: Box::new(NoopCommand), timestamp, macro_group });
            Some(command)
        } else {
            None
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn history(&self) -> &VecDeque<HistoryEntry> {
        &self.past
    }

    pub fn future_history(&self) -> &VecDeque<HistoryEntry> {
        &self.future
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }

    pub fn start_macro(&mut self) -> MacroId {
        let macro_id = MacroId(rand::random());
        self.macro_recording = Some(MacroRecorder::new(macro_id));
        self.macro_recording.as_mut().unwrap().start();
        macro_id
    }

    pub fn finish_macro(&mut self) -> Option<Box<dyn Command>> {
        if let Some(mut macro_rec) = self.macro_recording.take() {
            macro_rec.finish()
        } else {
            None
        }
    }

    pub fn is_macro_recording(&self) -> bool {
        self.macro_recording.as_ref().map_or(false, |m| m.is_recording())
    }

    fn create_snapshot(&self) {
        // Snapshot creation would serialize project state
        // This is a placeholder for the actual implementation
    }

    pub fn set_max_size(&mut self, size: usize) {
        self.max_size = size;
        while self.past.len() > size {
            self.past.pop_front();
        }
    }
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self::new(1000)
    }
}

pub struct UndoManager {
    history: Arc<Mutex<UndoHistory>>,
}

impl UndoManager {
    pub fn new(max_size: usize) -> Self {
        Self {
            history: Arc::new(Mutex::new(UndoHistory::new(max_size))),
        }
    }

    pub fn execute(&self, mut command: Box<dyn Command>) -> Result<()> {
        let mut history = self.history.lock().unwrap();
        command.execute(&mut crate::commands::CommandContext {
            project: &mut Project::new("temp"),
            engine_tx: &crossbeam::channel::unbounded().0,
        })?;
        history.push(command);
        Ok(())
    }

    pub fn undo(&self) -> Option<Box<dyn Command>> {
        let mut history = self.history.lock().unwrap();
        history.undo()
    }

    pub fn redo(&self) -> Option<Box<dyn Command>> {
        let mut history = self.history.lock().unwrap();
        history.redo()
    }

    pub fn can_undo(&self) -> bool {
        self.history.lock().unwrap().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.lock().unwrap().can_redo()
    }

    pub fn history(&self) -> Vec<String> {
        self.history.lock().unwrap().history().iter().map(|e| e.command.description()).collect()
    }

    pub fn start_macro(&self) -> MacroId {
        self.history.lock().unwrap().start_macro()
    }

    pub fn finish_macro(&self) -> Option<Box<dyn Command>> {
        self.history.lock().unwrap().finish_macro()
    }
}