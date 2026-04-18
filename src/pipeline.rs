/// A single stage in a shell pipeline.
#[derive(Debug, Clone)]
pub struct PipeStage {
    /// The shell command for this stage (e.g. "jq '.[]'")
    pub command: String,
    /// Timestamp of the most recent data chunk received from the executor.
    /// Used by the renderer to highlight the stage/pipe while data is actively flowing.
    pub last_update: Option<std::time::Instant>,
}

impl PipeStage {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            last_update: None,
        }
    }
}

/// The full pipeline (ordered list of stages).
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub stages: Vec<PipeStage>,
    /// Index of the currently selected stage.
    pub selected: usize,
}

impl Pipeline {
    pub fn new(stages: Vec<PipeStage>) -> Self {
        Self {
            stages,
            selected: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn selected_stage(&self) -> Option<&PipeStage> {
        self.stages.get(self.selected)
    }

    pub fn selected_stage_mut(&mut self) -> Option<&mut PipeStage> {
        self.stages.get_mut(self.selected)
    }

    /// Move selection to the next stage.
    pub fn select_next(&mut self) {
        if !self.stages.is_empty() && self.selected + 1 < self.stages.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the previous stage.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Insert a new empty stage after the currently selected stage.
    pub fn insert_after_selected(&mut self) {
        let pos = if self.stages.is_empty() {
            0
        } else {
            self.selected + 1
        };
        self.stages.insert(pos, PipeStage::new(""));
        self.selected = pos;
    }

    /// Remove the currently selected stage.
    pub fn remove_selected(&mut self) {
        if !self.stages.is_empty() {
            self.stages.remove(self.selected);
            if self.selected >= self.stages.len() && self.selected > 0 {
                self.selected -= 1;
            }
        }
    }

    pub fn from_commands(cmds: Vec<String>, parse: bool) -> Self {
        let mut stages: Vec<PipeStage> = vec![];
        let mut subcmds: Vec<String> = vec![];
        for cmd in cmds {
            if cmd.trim() == "|" {
                stages.push(PipeStage::new(join_shell_args(&subcmds)));
                subcmds.clear();
            } else if parse {
                // Flush any accumulated subcmds before splitting
                if !subcmds.is_empty() {
                    stages.push(PipeStage::new(subcmds.join(" ")));
                    subcmds.clear();
                }
                let parts = split_pipeline_stages(&cmd);
                for part in parts {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        stages.push(PipeStage::new(trimmed));
                    }
                }
            } else {
                subcmds.push(cmd);
            }
        }
        if !subcmds.is_empty() {
            stages.push(PipeStage::new(join_shell_args(&subcmds)));
        }
        Self::new(stages)
    }
}

// Just used in tests for easy pipeline creation from a single string like "a | b | c".
#[allow(dead_code)]
pub fn parse_pipeline(s: &str) -> Pipeline {
    return Pipeline::from_commands(vec![s.to_string()], true);
}

/// Shell-quote an argument if it contains characters that are special to the
/// shell.  This restores the quoting that the shell strips when it hands
/// individual words to the process via `argv`.
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg.bytes().all(|b| {
            matches!(b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'/' | b':' | b'@' | b'+' | b',' | b'%' | b'=')
        })
    {
        return arg.to_string();
    }
    // Wrap in single quotes; escape embedded single quotes.
    let escaped = arg.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn join_shell_args(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_pipeline_stages(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut prev_was_backslash = false;

    for (idx, ch) in s.char_indices() {
        match ch {
            '\\' if !prev_was_backslash => {
                prev_was_backslash = true;
                continue;
            }
            '\'' if !in_double_quote && !prev_was_backslash => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote && !prev_was_backslash => {
                in_double_quote = !in_double_quote;
            }
            '|' if !in_single_quote && !in_double_quote => {
                let before = idx.checked_sub(1).and_then(|i| s.as_bytes().get(i));
                let after = s.as_bytes().get(idx + 1);
                if before == Some(&b' ') && after == Some(&b' ') {
                    let stage_end = idx - 1;
                    parts.push(&s[start..stage_end]);
                    start = idx + 2;
                }
            }
            _ => {}
        }

        if ch != '\\' {
            prev_was_backslash = false;
        }
    }

    parts.push(&s[start..]);
    parts
}

#[cfg(test)]
#[path = "tests/pipeline.rs"]
mod tests;
