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
#[derive(Debug)]
pub struct Pipeline {
    pub stages: Vec<PipeStage>,
    /// Index of the currently selected stage.
    pub selected: usize,
}

impl Pipeline {
    pub fn new(stages: Vec<PipeStage>) -> Self {
        Self { stages, selected: 0 }
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
    let stages: Vec<PipeStage> = s
        .split(" | ")
        .map(|part| PipeStage::new(part.trim()))
        .filter(|s| !s.command.is_empty())
        .collect();
    Pipeline::new(stages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pipeline() {
        let p = parse_pipeline("echo hello | grep hello | wc -l");
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.stages[0].command, "echo hello");
        assert_eq!(p.stages[1].command, "grep hello");
        assert_eq!(p.stages[2].command, "wc -l");
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
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.selected, 1);
        p.remove_selected();
        assert_eq!(p.stages.len(), 2);
    }
}
