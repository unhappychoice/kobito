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
        let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
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
    use std::path::PathBuf;

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kobito-tasks-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    #[test]
    fn from_file_reads_existing_backlog() {
        let dir = unique_dir("from_file_ok");
        let path = dir.join("tasks.md");
        fs::write(&path, "- [ ] alpha\n- [x] beta\n").unwrap();

        let b = Backlog::from_file(&path).expect("from_file should succeed");
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.items[0].body, "alpha");
        assert!(!b.items[0].completed);
        assert!(b.items[1].completed);
    }

    #[test]
    fn from_file_returns_error_when_path_missing() {
        let dir = unique_dir("from_file_missing");
        let path = dir.join("nope.md");

        let err = Backlog::from_file(&path).expect_err("missing file should error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("read") && msg.contains("nope.md"),
            "error should mention read context and path: {msg}",
        );
    }

    #[test]
    fn write_persists_raw_back_to_disk() {
        let dir = unique_dir("write");
        let path = dir.join("out.md");
        let mut b = Backlog::parse("- [ ] todo\n- [ ] also\n");
        b.mark_completed(0);
        b.write(&path).expect("write should succeed");

        let read_back = fs::read_to_string(&path).unwrap();
        assert!(read_back.contains("- [x] todo"));
        assert!(read_back.contains("- [ ] also"));
        assert!(read_back.ends_with('\n'));
    }

    #[test]
    fn mark_completed_is_a_noop_for_out_of_range_line() {
        let mut b = Backlog::parse("- [ ] one\n- [ ] two\n");
        let before = b.raw.clone();
        b.mark_completed(99);
        assert_eq!(b.raw, before);
        assert!(!b.items[0].completed);
        assert!(!b.items[1].completed);
    }

    #[test]
    fn mark_completed_preserves_absence_of_trailing_newline() {
        let mut b = Backlog::parse("- [ ] one\n- [ ] two");
        b.mark_completed(1);
        assert!(b.raw.contains("- [x] two"));
        assert!(!b.raw.ends_with('\n'));
    }
}
