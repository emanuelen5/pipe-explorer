use regex::RegexBuilder;

/// All state related to the incremental output search.
#[derive(Debug, Default)]
pub struct SearchState {
    /// The current search query (may include \c/\C modifiers).
    pub query: String,
    /// Cursor position within the query (byte index).
    pub cursor: usize,
    /// All matches in the current output: (line_index, start_byte, end_byte).
    pub matches: Vec<(usize, usize, usize)>,
    /// Index of the currently highlighted match.
    pub match_idx: usize,
}

impl SearchState {
    /// A const empty search state for use as a static default.
    pub const fn empty() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            match_idx: 0,
        }
    }
    /// Parse a Vim-style search query, extracting the regex pattern and case flag.
    /// `\c` anywhere in the pattern → case-insensitive; `\C` → case-sensitive (default).
    pub fn parse_vim_pattern(query: &str) -> (String, bool) {
        let mut pattern = query.to_string();
        let contains_upper_case_char = pattern.chars().any(|c| c.is_ascii_uppercase());
        let case_sensitive: bool;
        if pattern.contains("\\c") {
            case_sensitive = false;
            pattern = pattern.replace("\\c", "");
        } else if pattern.contains("\\C") {
            case_sensitive = true;
            pattern = pattern.replace("\\C", "");
        } else {
            case_sensitive = contains_upper_case_char;
        }
        (pattern, case_sensitive)
    }

    /// Recompute matches against `content` using the current `query`.
    /// Resets `match_idx` to 0. Invalid regex patterns are silently ignored.
    pub fn compute(&mut self, content: &str) {
        self.matches.clear();
        self.match_idx = 0;
        if self.query.is_empty() {
            return;
        }
        let (pattern, case_sensitive) = Self::parse_vim_pattern(&self.query);
        if pattern.is_empty() {
            return;
        }
        let re = match RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(r) => r,
            Err(_) => return,
        };
        for (line_idx, line) in content.lines().enumerate() {
            for m in re.find_iter(line) {
                self.matches.push((line_idx, m.start(), m.end()));
            }
        }
    }

    /// Reset all search state.
    pub fn clear(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.match_idx = 0;
    }
}

/// History of committed search queries, shared across all pipeline stages.
///
/// The history is deduplicated (most recent at index 0). Once stored, entries
/// are immutable: navigating with the arrow keys shows a local copy of each
/// entry that can be modified freely, but those modifications are never written
/// back. Pressing `Alt+R` while on a history entry reverts to the original.
/// Modifications are discarded when a new search is confirmed or when search
/// mode is entered again.
#[derive(Debug, Default)]
pub struct SearchHistory {
    /// Committed entries, newest first.
    entries: Vec<String>,
    /// Current navigation position. `None` means we are editing a fresh draft.
    nav_index: Option<usize>,
    /// The in-progress text the user was typing before any history navigation.
    draft: String,
    /// Temporary per-entry edits (keyed by entry index). Never written back to
    /// `entries`.
    modifications: std::collections::HashMap<usize, String>,
}

#[cfg(test)]
impl SearchHistory {
    /// Return the number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if there are no stored entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl SearchHistory {
    /// Push `query` to the front of the history, deduplicating it.
    ///
    /// Empty queries are silently ignored. Also resets navigation so the next
    /// search starts with a blank draft rather than at a history position.
    pub fn push(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }
        self.entries.retain(|e| e != query);
        self.entries.insert(0, query.to_string());
        self.reset_navigation();
    }

    /// Reset the navigation cursor back to the draft position.
    ///
    /// Call this when entering search mode so history traversal always starts
    /// from a blank draft rather than where the user last left off.
    pub fn reset_navigation(&mut self) {
        self.nav_index = None;
        self.draft.clear();
        self.modifications.clear();
    }

    /// Navigate to the previous (older) history entry.
    ///
    /// `current_text` is the text currently visible in the search box. It is
    /// saved as the draft (when moving away from the draft position) or as a
    /// temporary modification for the current history entry (when already
    /// navigating history).
    ///
    /// Returns the text that should be placed in the search box, or `None`
    /// when the history is empty or we are already at the oldest entry.
    pub fn navigate_up(&mut self, current_text: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.nav_index {
            None => {
                // Save draft and move to the most recent entry.
                self.draft = current_text.to_string();
                self.nav_index = Some(0);
            }
            Some(i) if i + 1 < self.entries.len() => {
                // Save a temporary modification for this entry and go older.
                self.modifications.insert(i, current_text.to_string());
                self.nav_index = Some(i + 1);
            }
            Some(_) => {
                // Already at the oldest entry; do nothing.
                return None;
            }
        }
        let idx = self.nav_index.unwrap();
        Some(
            self.modifications
                .get(&idx)
                .cloned()
                .unwrap_or_else(|| self.entries[idx].clone()),
        )
    }

    /// Navigate to the next (newer) history entry, or back to the draft.
    ///
    /// `current_text` is saved as a temporary modification for the current
    /// entry before moving forward.
    ///
    /// Returns the text that should be placed in the search box, or `None`
    /// when already at the draft position.
    pub fn navigate_down(&mut self, current_text: &str) -> Option<String> {
        let i = self.nav_index?;
        // Save a temporary modification before moving away.
        self.modifications.insert(i, current_text.to_string());
        if i == 0 {
            // Return to draft.
            self.nav_index = None;
            Some(self.draft.clone())
        } else {
            let new_idx = i - 1;
            self.nav_index = Some(new_idx);
            Some(
                self.modifications
                    .get(&new_idx)
                    .cloned()
                    .unwrap_or_else(|| self.entries[new_idx].clone()),
            )
        }
    }

    /// Revert the currently displayed history entry to its original stored
    /// value, discarding any temporary modification.
    ///
    /// Returns the original text, or `None` when at the draft position (no
    /// history entry selected).
    pub fn revert_current(&mut self) -> Option<String> {
        let idx = self.nav_index?;
        self.modifications.remove(&idx);
        Some(self.entries[idx].clone())
    }

    /// Get the most recent search
    pub fn last(&self) -> Option<String> {
        self.entries.last().cloned()
    }
}

#[cfg(test)]
mod tests;
