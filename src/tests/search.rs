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
    h.navigate_up(""); // at "new"
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
    h.push("bar"); // should reset navigation
    // navigate_down from draft returns None (at draft).
    assert_eq!(h.navigate_down(""), None);
}
