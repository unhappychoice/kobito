use indicatif::{ProgressBar, ProgressStyle};
use std::io::Write;
use std::time::Duration;

/// Hides the terminal cursor for the duration of its scope.
///
/// Indicatif's spinner already paints over the cursor most of the time,
/// but the cursor flashes through between bar redraws. Hide it on
/// construction, restore it on drop — works for normal exits, panics,
/// and the explicit drop in the Ctrl-C handler.
pub struct CursorGuard;

impl CursorGuard {
    pub fn new() -> Self {
        print!("\x1b[?25l");
        let _ = std::io::stdout().flush();
        Self
    }
}

impl Drop for CursorGuard {
    fn drop(&mut self) {
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
}

pub fn make_status_bar() -> ProgressBar {
    let bar = ProgressBar::new_spinner();
    // Two-line template:
    //   1. a thin separator rule, dimmed
    //   2. the actual status row, painted on the kobito-channel BG
    //      to end-of-line via \x1b[K so the bar reads as a sticky
    //      footer rather than a floating spinner.
    //
    // {elapsed_precise} is owned by indicatif so the seconds tick on
    // every steady_tick (every 120 ms below) without us re-calling
    // set_message.
    let rule = "\x1b[38;2;90;95;105m\
        ──────────────────────────────────────────────────────────────────\
        \x1b[0m";
    let template = format!(
        "{rule}\n\
         \x1b[48;2;25;30;42m\x1b[38;2;180;200;225m \
         {{spinner}}  iter {{prefix}}  ·  {{elapsed_precise}}  ·  {{msg}} \
         \x1b[K\x1b[0m",
    );
    bar.set_style(
        ProgressStyle::with_template(&template)
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    bar.enable_steady_tick(Duration::from_millis(120));
    bar
}

pub fn set_status(bar: &ProgressBar, iteration: u32, retries: u32, state: &str) {
    bar.set_prefix(iteration.to_string());
    bar.set_message(format!("retry {retries}  ·  {state}"));
}
