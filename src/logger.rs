use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use indicatif::ProgressBar;

#[derive(Serialize)]
struct LogLine<'a> {
    ts: String,
    source: &'a str,
    line: &'a str,
}

#[derive(Clone)]
pub struct LogSink {
    inner: Arc<Mutex<File>>,
    bar: Option<ProgressBar>,
}

impl LogSink {
    pub fn open(log_path: &Path, bar: Option<ProgressBar>) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
            bar,
        })
    }

    pub fn write(&self, source: &str, line: &str) {
        let entry = LogLine {
            ts: Utc::now().to_rfc3339(),
            source,
            line,
        };
        if let Ok(json) = serde_json::to_string(&entry) {
            if let Ok(mut f) = self.inner.lock() {
                let _ = writeln!(f, "{json}");
            }
        }
        if let Some(bar) = &self.bar {
            bar.println(format!("│ {line}"));
        } else {
            println!("{line}");
        }
    }

    pub fn note(&self, line: &str) {
        self.write("kobito", line);
    }
}
