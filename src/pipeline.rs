/// A single stage in a shell pipeline.
#[derive(Debug, Clone)]
pub struct PipeStage {
    /// The shell command for this stage (e.g. "jq '.[]'")
    pub command: String,
}

impl PipeStage {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
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
}

/// Parse a pipeline string (e.g. "cmd1 | cmd2 | cmd3") into a Pipeline.
pub fn parse_pipeline(s: &str) -> Pipeline {
    let stages: Vec<PipeStage> = split_pipeline_stages(s)
        .into_iter()
        .map(|part| PipeStage::new(part.trim()))
        .filter(|s| !s.command.is_empty())
        .collect();
    Pipeline::new(stages)
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
