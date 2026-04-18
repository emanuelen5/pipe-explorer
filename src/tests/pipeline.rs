use super::*;

#[test]
fn test_parse_pipeline() {
    let p = parse_pipeline("echo hello | grep hello | wc -l");
    assert_eq!(p.stages[0].command, "echo hello");
    assert_eq!(p.stages[1].command, "grep hello");
    assert_eq!(p.stages[2].command, "wc -l");
    assert_eq!(p.stages.len(), 3);
}

#[test]
fn test_parse_pipeline_ignores_pipe_inside_single_quotes() {
    let p = parse_pipeline(
        "gh api repos/emanuelen5/pipe-explorer/commits | jq '.[] | (.sha, .commit.verification.verified)'",
    );
    assert_eq!(
        p.stages[0].command,
        "gh api repos/emanuelen5/pipe-explorer/commits"
    );
    assert_eq!(
        p.stages[1].command,
        "jq '.[] | (.sha, .commit.verification.verified)'"
    );
    assert_eq!(p.stages.len(), 2);
}

#[test]
fn test_parse_pipeline_ignores_pipe_inside_double_quotes() {
    let p = parse_pipeline("printf \"a | b\" | wc -c");
    assert_eq!(p.stages[0].command, "printf \"a | b\"");
    assert_eq!(p.stages[1].command, "wc -c");
    assert_eq!(p.stages.len(), 2);
}

#[test]
fn test_navigate() {
    let mut p = parse_pipeline("a | b | c");
    assert_eq!(p.selected, 0);
    p.select_next();
    assert_eq!(p.selected, 1);
    p.select_prev();
    assert_eq!(p.selected, 0);
    // Cannot go before 0
    p.select_prev();
    assert_eq!(p.selected, 0);
}

#[test]
fn test_insert_remove() {
    let mut p = parse_pipeline("a | b");
    p.insert_after_selected();
    assert_eq!(p.selected, 1);
    assert_eq!(p.stages.len(), 3);
    p.remove_selected();
    assert_eq!(p.stages.len(), 2);
}
