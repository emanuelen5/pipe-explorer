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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vim_pattern_default_case_sensitive() {
        let (pat, ci) = SearchState::parse_vim_pattern("hello");
        assert_eq!(pat, "hello");
        assert!(!ci);
    }

    #[test]
    fn test_parse_vim_pattern_case_insensitive() {
        let (pat, ci) = SearchState::parse_vim_pattern("hello\\c");
        assert_eq!(pat, "hello");
        assert!(!ci);
    }

    #[test]
    fn test_parse_vim_pattern_explicit_case_sensitive() {
        let (pat, ci) = SearchState::parse_vim_pattern("hello\\C");
        assert_eq!(pat, "hello");
        assert!(ci);
    }

    #[test]
    fn test_parse_vim_pattern_case_sensitive_due_to_uppercase() {
        let (pat, ci) = SearchState::parse_vim_pattern("Hello");
        assert_eq!(pat, "Hello");
        assert!(ci);
    }

    #[test]
    fn test_compute_basic() {
        let mut s = SearchState::default();
        s.query = "hello".to_string();
        s.compute("hello world\nhello rust\ngoodbye");
        assert_eq!(s.matches.len(), 2);
        assert_eq!(s.matches[0].0, 0);
        assert_eq!(s.matches[1].0, 1);
    }

    #[test]
    fn test_compute_no_match() {
        let mut s = SearchState::default();
        s.query = "xyz".to_string();
        s.compute("hello world");
        assert!(s.matches.is_empty());
    }

    #[test]
    fn test_compute_case_insensitive() {
        let mut s = SearchState::default();
        s.query = "hello\\c".to_string();
        s.compute("Hello World\nhello");
        assert_eq!(s.matches.len(), 2);
    }

    #[test]
    fn test_compute_case_sensitive_modifier() {
        let mut s = SearchState::default();
        s.query = "hello\\C".to_string();
        s.compute("Hello World\nhello");
        assert_eq!(s.matches.len(), 1);
        assert_eq!(s.matches[0].0, 1);
    }

    #[test]
    fn test_compute_regex() {
        let mut s = SearchState::default();
        s.query = r"\d+".to_string();
        s.compute("foo123\nbar456\nbaz");
        assert_eq!(s.matches.len(), 2);
    }

    #[test]
    fn test_compute_invalid_regex() {
        let mut s = SearchState::default();
        s.query = "[invalid".to_string();
        s.compute("hello");
        assert!(s.matches.is_empty());
    }

    #[test]
    fn test_compute_empty_query() {
        let mut s = SearchState::default();
        s.compute("hello world");
        assert!(s.matches.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut s = SearchState::default();
        s.query = "hello".to_string();
        s.cursor = 5;
        s.compute("hello");
        assert!(!s.matches.is_empty());
        s.clear();
        assert!(s.query.is_empty());
        assert_eq!(s.cursor, 0);
        assert!(s.matches.is_empty());
        assert_eq!(s.match_idx, 0);
    }

    // ----- SearchHistory tests -----

    #[test]
    fn test_history_push_stores_entry() {
        let mut h = SearchHistory::default();
        h.push("foo");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn test_history_push_empty_is_ignored() {
        let mut h = SearchHistory::default();
        h.push("");
        assert!(h.is_empty());
    }

    #[test]
    fn test_history_push_deduplicates() {
        let mut h = SearchHistory::default();
        h.push("foo");
        h.push("bar");
        h.push("foo");
        // "foo" should appear only once, as the most recent entry.
        assert_eq!(h.len(), 2);
        // Most recent is at index 0; navigating up should return "foo".
        assert_eq!(h.navigate_up(""), Some("foo".to_string()));
    }

    #[test]
    fn test_history_navigate_up_empty_history_returns_none() {
        let mut h = SearchHistory::default();
        assert_eq!(h.navigate_up("current"), None);
    }

    #[test]
    fn test_history_navigate_up_saves_draft() {
        let mut h = SearchHistory::default();
        h.push("bar");
        // Navigate up from draft "foo" → should get "bar".
        let result = h.navigate_up("foo");
        assert_eq!(result, Some("bar".to_string()));
        // Navigate down → should restore "foo".
        let back = h.navigate_down("bar");
        assert_eq!(back, Some("foo".to_string()));
    }

    #[test]
    fn test_history_navigate_up_multiple_entries() {
        let mut h = SearchHistory::default();
        h.push("old");
        h.push("new");
        // "new" is at index 0, "old" at index 1.
        assert_eq!(h.navigate_up(""), Some("new".to_string()));
        assert_eq!(h.navigate_up("new"), Some("old".to_string()));
    }

    #[test]
    fn test_history_navigate_up_at_oldest_returns_none() {
        let mut h = SearchHistory::default();
        h.push("only");
        h.navigate_up("");
        // Already at oldest; another up should return None.
        assert_eq!(h.navigate_up("only"), None);
    }

    #[test]
    fn test_history_navigate_down_from_draft_returns_none() {
        let mut h = SearchHistory::default();
        h.push("foo");
        assert_eq!(h.navigate_down("typing"), None);
    }

    #[test]
    fn test_history_navigate_down_restores_modification() {
        let mut h = SearchHistory::default();
        h.push("old");
        h.push("new");
        // Go up twice to reach "old".
        h.navigate_up("");     // at "new"
        h.navigate_up("new"); // at "old"
        // Modify "old" → "old_modified", then go down.
        let result = h.navigate_down("old_modified");
        // Should show the modified version of "new" (none yet, so original "new").
        assert_eq!(result, Some("new".to_string()));
        // Go down again → draft.
        let draft = h.navigate_down("new");
        assert_eq!(draft, Some("".to_string()));
    }

    #[test]
    fn test_history_navigate_remembers_modifications_across_traversal() {
        let mut h = SearchHistory::default();
        h.push("old");
        h.push("new");
        // Navigate up to "new", modify it.
        h.navigate_up("draft_text");
        // Navigate up again (saves "modified_new" for "new"), go to "old".
        h.navigate_up("modified_new");
        // Navigate down back to "new" (saves current "old" text for "old").
        let back_to_new = h.navigate_down("old");
        assert_eq!(back_to_new, Some("modified_new".to_string()));
    }

    #[test]
    fn test_history_revert_current_restores_original() {
        let mut h = SearchHistory::default();
        h.push("original");
        // Navigate up to "original", then modify it.
        h.navigate_up("draft");
        // Revert should return the stored "original".
        let reverted = h.revert_current();
        assert_eq!(reverted, Some("original".to_string()));
        // After revert, navigate_up again should still return "original" (not modified).
        // (We've exhausted history, so up returns None; navigating back down gives draft.)
        let back = h.navigate_down("original");
        assert_eq!(back, Some("draft".to_string()));
    }

    #[test]
    fn test_history_revert_at_draft_position_returns_none() {
        let mut h = SearchHistory::default();
        h.push("foo");
        assert_eq!(h.revert_current(), None);
    }

    #[test]
    fn test_history_reset_navigation_clears_state() {
        let mut h = SearchHistory::default();
        h.push("foo");
        h.navigate_up("draft");
        h.reset_navigation();
        // After reset, navigate_down from draft position should return None.
        assert_eq!(h.navigate_down(""), None);
        // Entries are still there.
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn test_history_push_resets_navigation() {
        let mut h = SearchHistory::default();
        h.push("foo");
        h.navigate_up("draft"); // at "foo"
        h.push("bar");          // should reset navigation
        // navigate_down from draft returns None (at draft).
        assert_eq!(h.navigate_down(""), None);
    }
}
