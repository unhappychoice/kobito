use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub line_no: usize,
    pub completed: bool,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Backlog {
    pub raw: String,
    pub items: Vec<Task>,
}

impl Backlog {
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        Ok(Self::parse(&raw))
    }

    pub fn parse(raw: &str) -> Self {
        let re = pattern();
        let items = raw
            .lines()
            .enumerate()
            .filter_map(|(line_no, line)| {
                re.captures(line).map(|c| Task {
                    line_no,
                    completed: &c[1] != " ",
                    body: c[2].to_string(),
                })
            })
            .collect();
        Self {
            raw: raw.to_string(),
            items,
        }
    }

    pub fn pending(&self) -> Vec<&Task> {
        self.items.iter().filter(|t| !t.completed).collect()
    }

    pub fn mark_completed(&mut self, line_no: usize) {
        let trailing = self.raw.ends_with('\n');
        let mut lines: Vec<String> = self.raw.lines().map(|s| s.to_string()).collect();
        if let Some(line) = lines.get_mut(line_no) {
            *line = pattern().replace(line, "- [x] $2").to_string();
        }
        let mut joined = lines.join("\n");
        if trailing {
            joined.push('\n');
        }
        self.raw = joined;
        if let Some(item) = self.items.iter_mut().find(|t| t.line_no == line_no) {
            item.completed = true;
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        fs::write(path, &self.raw)?;
        Ok(())
    }
}

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^- \[( |x|X)\] (.*)$").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_checkboxes_and_skips_other_lines() {
        let raw = "# Backlog\n\n- [ ] one\n- [x] two\nrandom line\n- [ ] three\n";
        let b = Backlog::parse(raw);
        assert_eq!(b.items.len(), 3);
        assert_eq!(b.items[0].body, "one");
        assert!(b.items[1].completed);
        assert!(!b.items[2].completed);
        assert_eq!(b.items[2].line_no, 5);
    }

    #[test]
    fn mark_completed_rewrites_target_line_only() {
        let mut b = Backlog::parse("- [ ] one\n- [ ] two\n- [ ] three\n");
        b.mark_completed(1);
        assert!(b.raw.contains("- [x] two"));
        assert!(b.raw.contains("- [ ] one"));
        assert!(b.raw.contains("- [ ] three"));
        assert!(b.items[1].completed);
        assert!(!b.items[0].completed);
    }

    #[test]
    fn pending_excludes_completed() {
        let b = Backlog::parse("- [x] done\n- [ ] todo\n");
        let pending = b.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].body, "todo");
    }

    #[test]
    fn round_trip_preserves_trailing_newline() {
        let mut b = Backlog::parse("- [ ] one\n");
        b.mark_completed(0);
        assert!(b.raw.ends_with('\n'));
    }
}
