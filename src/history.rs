use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Maximum number of history entries to keep.
const MAX_ENTRIES: usize = 200;

/// A single pipeline history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The pipeline commands (one per stage).
    pub commands: Vec<String>,
    /// Unix timestamp (seconds) when this pipeline was last used.
    pub timestamp: u64,
}

/// The full history state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    /// Return the path to the history file: `~/.pipe-explorer/history.json`.
    pub fn path() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".pipe-explorer").join("history.json"))
    }

    /// Load history from disk. Returns an empty history if the file doesn't exist
    /// or can't be parsed.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(data) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    /// Save history to disk. Creates the directory if needed.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(data) = serde_json::to_string_pretty(self) else {
            return;
        };
        let _ = fs::write(&path, data);
    }

    /// Add a pipeline to the history. If an identical pipeline already exists,
    /// updates its timestamp and moves it to the front. Otherwise inserts a new
    /// entry at the front. Caps at MAX_ENTRIES.
    pub fn add(&mut self, commands: &[String]) {
        // Skip empty pipelines.
        if commands.is_empty() || commands.iter().all(|c| c.trim().is_empty()) {
            return;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Remove duplicate if it exists.
        self.entries.retain(|e| e.commands != commands);

        // Insert at the front (most recent).
        self.entries.insert(
            0,
            HistoryEntry {
                commands: commands.to_vec(),
                timestamp: now,
            },
        );

        // Trim to max entries.
        self.entries.truncate(MAX_ENTRIES);
    }

    /// Get the Nth most recent entry (0-indexed).
    pub fn get(&self, index: usize) -> Option<&HistoryEntry> {
        self.entries.get(index)
    }

    /// Remove the entry at the given index (0-indexed).
    /// Returns true if an entry was removed.
    pub fn remove(&mut self, index: usize) -> bool {
        if index < self.entries.len() {
            self.entries.remove(index);
            true
        } else {
            false
        }
    }

    /// Format history for display to the user.
    pub fn display(&self) -> String {
        if self.entries.is_empty() {
            return "No recent pipelines.".to_string();
        }
        let mut out = String::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let pipeline_str = entry.commands.join(" | ");
            out.push_str(&format!("  {:>2}  {}\n", i + 1, pipeline_str));
        }
        out
    }
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
#[path = "tests/history.rs"]
mod tests;
