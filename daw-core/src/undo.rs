//! Undo/redo history.
//!
//! [`History`] is a small, action-type-agnostic stack used by the UI and any
//! other layer that wants linear undo with a capacity limit. The command-based
//! [`UndoHistory`] builds on it for `dyn Command` edits.

use crate::commands::{Command, CommandContext, MacroCommand};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// One entry in a [`History`]: the caller's action plus a human-readable label.
pub struct HistoryEntry<T> {
    pub action: T,
    pub description: String,
    pub timestamp: u64,
}

/// Linear undo/redo history with a capacity limit.
///
/// The history stores and moves actions; applying them (and any UI side
/// effects) stays with the caller. Typical use:
///
/// ```ignore
/// if let Some(entry) = history.take_undo() {
///     apply(&entry.action, false);
///     history.complete_undo(entry);
/// }
/// ```
pub struct History<T> {
    past: Vec<HistoryEntry<T>>,
    future: Vec<HistoryEntry<T>>,
    max_size: usize,
}

impl<T> History<T> {
    pub fn new(max_size: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            max_size: max_size.max(1),
        }
    }

    /// Record a new action, discarding any redo branch.
    pub fn push(&mut self, action: T, description: impl Into<String>) {
        self.future.clear();
        self.past.push(HistoryEntry {
            action,
            description: description.into(),
            timestamp: now_millis(),
        });
        self.trim();
    }

    /// Remove the most recent action so the caller can apply its undo.
    /// Call [`complete_undo`](Self::complete_undo) afterwards.
    pub fn take_undo(&mut self) -> Option<HistoryEntry<T>> {
        self.past.pop()
    }

    /// Move an action taken by [`take_undo`](Self::take_undo) onto the redo
    /// stack.
    pub fn complete_undo(&mut self, entry: HistoryEntry<T>) {
        self.future.insert(0, entry);
    }

    /// Remove the next redo action so the caller can apply it.
    /// Call [`complete_redo`](Self::complete_redo) afterwards.
    pub fn take_redo(&mut self) -> Option<HistoryEntry<T>> {
        if self.future.is_empty() {
            None
        } else {
            Some(self.future.remove(0))
        }
    }

    /// Move an action taken by [`take_redo`](Self::take_redo) back onto the
    /// undo stack.
    pub fn complete_redo(&mut self, entry: HistoryEntry<T>) {
        self.past.push(entry);
        self.trim();
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn past_len(&self) -> usize {
        self.past.len()
    }

    pub fn future_len(&self) -> usize {
        self.future.len()
    }

    pub fn past(&self) -> &[HistoryEntry<T>] {
        &self.past
    }

    pub fn future(&self) -> &[HistoryEntry<T>] {
        &self.future
    }

    pub fn last_mut(&mut self) -> Option<&mut HistoryEntry<T>> {
        self.past.last_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.past.is_empty() && self.future.is_empty()
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn set_max_size(&mut self, size: usize) {
        self.max_size = size.max(1);
        self.trim();
    }

    fn trim(&mut self) {
        while self.past.len() > self.max_size {
            self.past.remove(0);
        }
    }
}

/// Command-backed undo history for the core layer.
pub struct UndoHistory {
    history: History<Box<dyn Command>>,
    macro_recording: Option<MacroRecorder>,
}

impl UndoHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            history: History::new(max_size),
            macro_recording: None,
        }
    }

    /// Execute `command` and record it as one undoable step.
    pub fn execute(
        &mut self,
        mut command: Box<dyn Command>,
        ctx: &mut CommandContext,
    ) -> Result<()> {
        command.execute(ctx)?;
        let description = command.description();
        self.history.push(command, description);
        Ok(())
    }

    /// Record a command that has already been applied.
    pub fn push(&mut self, command: Box<dyn Command>) {
        let description = command.description();
        self.history.push(command, description);
    }

    pub fn undo(&mut self, ctx: &mut CommandContext) -> Result<bool> {
        let Some(mut entry) = self.history.take_undo() else {
            return Ok(false);
        };
        entry.action.undo(ctx)?;
        self.history.complete_undo(entry);
        Ok(true)
    }

    pub fn redo(&mut self, ctx: &mut CommandContext) -> Result<bool> {
        let Some(mut entry) = self.history.take_redo() else {
            return Ok(false);
        };
        entry.action.execute(ctx)?;
        self.history.complete_redo(entry);
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    pub fn descriptions(&self) -> Vec<String> {
        self.history
            .past()
            .iter()
            .map(|entry| entry.description.clone())
            .collect()
    }

    pub fn set_max_size(&mut self, size: usize) {
        self.history.set_max_size(size);
    }

    pub fn start_macro(&mut self) -> MacroId {
        let macro_id = MacroId(rand::random());
        let mut recorder = MacroRecorder::new(macro_id);
        recorder.start();
        self.macro_recording = Some(recorder);
        macro_id
    }

    /// Finish the active macro and return the grouped command, if any.
    pub fn finish_macro(&mut self) -> Option<Box<dyn Command>> {
        self.macro_recording
            .take()
            .and_then(|mut recorder| recorder.finish())
    }

    pub fn is_macro_recording(&self) -> bool {
        self.macro_recording
            .as_ref()
            .is_some_and(MacroRecorder::is_recording)
    }
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self::new(1000)
    }
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
            Some(Box::new(MacroCommand {
                commands: std::mem::take(&mut self.commands),
            }))
        } else {
            self.commands.pop()
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording
    }

    pub fn macro_id(&self) -> MacroId {
        self.macro_id
    }
}

/// Shared, thread-safe undo history for callers that need to hand commands
/// between threads.
pub struct UndoManager {
    history: Arc<Mutex<UndoHistory>>,
}

impl UndoManager {
    pub fn new(max_size: usize) -> Self {
        Self {
            history: Arc::new(Mutex::new(UndoHistory::new(max_size))),
        }
    }

    pub fn execute(&self, command: Box<dyn Command>, ctx: &mut CommandContext) -> Result<()> {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .execute(command, ctx)
    }

    pub fn undo(&self, ctx: &mut CommandContext) -> Result<bool> {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .undo(ctx)
    }

    pub fn redo(&self, ctx: &mut CommandContext) -> Result<bool> {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .redo(ctx)
    }

    pub fn can_undo(&self) -> bool {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .can_redo()
    }

    pub fn history(&self) -> Vec<String> {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .descriptions()
    }

    pub fn start_macro(&self) -> MacroId {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .start_macro()
    }

    pub fn finish_macro(&self) -> Option<Box<dyn Command>> {
        self.history
            .lock()
            .expect("undo history lock poisoned")
            .finish_macro()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn history_moves_entries_between_undo_and_redo() {
        let mut history = History::new(10);
        history.push(1, "one");
        history.push(2, "two");

        let entry = history.take_undo().expect("entry");
        assert_eq!(entry.action, 2);
        history.complete_undo(entry);
        assert_eq!(history.past_len(), 1);
        assert_eq!(history.future_len(), 1);

        let entry = history.take_redo().expect("entry");
        assert_eq!(entry.action, 2);
        history.complete_redo(entry);
        assert_eq!(history.past_len(), 2);
        assert_eq!(history.future_len(), 0);
    }

    #[test]
    fn history_capacity_drops_oldest_entries() {
        let mut history = History::new(2);
        history.push(1, "one");
        history.push(2, "two");
        history.push(3, "three");

        assert_eq!(history.past_len(), 2);
        assert_eq!(history.past()[0].action, 2);
        assert_eq!(history.past()[1].action, 3);
    }

    #[test]
    fn new_edits_clear_the_redo_branch() {
        let mut history = History::new(10);
        history.push(1, "one");
        history.push(2, "two");
        let entry = history.take_undo().expect("entry");
        history.complete_undo(entry);
        assert_eq!(history.future_len(), 1);

        history.push(3, "three");
        assert_eq!(history.future_len(), 0);
        assert_eq!(history.past_len(), 2);
    }

    #[test]
    fn set_max_size_trims_history() {
        let mut history = History::new(10);
        for value in 0..5 {
            history.push(value, format!("edit {value}"));
        }
        history.set_max_size(3);
        assert_eq!(history.past_len(), 3);
        assert_eq!(history.past()[0].action, 2);
    }

    struct Counter {
        count: Arc<AtomicUsize>,
    }

    impl Command for Counter {
        fn execute(&mut self, _ctx: &mut CommandContext) -> Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn undo(&mut self, _ctx: &mut CommandContext) -> Result<()> {
            self.count.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }

        fn description(&self) -> String {
            "Count".to_string()
        }
    }

    #[test]
    fn command_history_executes_undoes_and_redoes() {
        let mut project = Project::new("test");
        let (engine_tx, _engine_rx) = crossbeam::channel::unbounded();
        let count = Arc::new(AtomicUsize::new(0));
        let mut history = UndoHistory::new(10);

        history
            .execute(
                Box::new(Counter {
                    count: count.clone(),
                }),
                &mut CommandContext {
                    project: &mut project,
                    engine_tx: &engine_tx,
                },
            )
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(history.can_undo());

        history
            .undo(&mut CommandContext {
                project: &mut project,
                engine_tx: &engine_tx,
            })
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(history.can_redo());

        history
            .redo(&mut CommandContext {
                project: &mut project,
                engine_tx: &engine_tx,
            })
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
