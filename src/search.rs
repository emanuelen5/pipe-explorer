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
}
