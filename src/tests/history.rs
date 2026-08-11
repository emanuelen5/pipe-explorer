use super::*;

#[test]
fn add_and_retrieve() {
    let mut history = History::default();
    let cmds = vec!["echo hello".to_string(), "grep hello".to_string()];
    history.add(&cmds);
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].commands, cmds);
}

#[test]
fn deduplicates_same_pipeline() {
    let mut history = History::default();
    let cmds = vec!["echo hello".to_string()];
    history.add(&cmds);
    history.add(&cmds);
    assert_eq!(history.entries.len(), 1);
}

#[test]
fn most_recent_first() {
    let mut history = History::default();
    history.add(&vec!["first".to_string()]);
    history.add(&vec!["second".to_string()]);
    assert_eq!(history.entries[0].commands, vec!["second".to_string()]);
    assert_eq!(history.entries[1].commands, vec!["first".to_string()]);
}

#[test]
fn skips_empty_pipelines() {
    let mut history = History::default();
    history.add(&vec![]);
    history.add(&vec!["  ".to_string()]);
    assert_eq!(history.entries.len(), 0);
}

#[test]
fn caps_at_max_entries() {
    let mut history = History::default();
    for i in 0..250 {
        history.add(&vec![format!("cmd {}", i)]);
    }
    assert!(history.entries.len() <= MAX_ENTRIES);
}

#[test]
fn display_empty() {
    let history = History::default();
    assert_eq!(history.display(), "No recent pipelines.");
}

#[test]
fn display_nonempty() {
    let mut history = History::default();
    history.add(&vec!["echo hi".to_string(), "wc -l".to_string()]);
    let output = history.display();
    assert!(output.contains("echo hi | wc -l"));
}
