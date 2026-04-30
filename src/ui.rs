use console::Term;
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
    //   1. a separator rule painted across the full terminal width on
    //      the kobito-channel BG so the rule itself is part of the
    //      footer block, not a free-floating line of dashes.
    //   2. the status row, also on the kobito BG. {wide_msg} fills
    //      whatever horizontal space is left so the BG continues to
    //      end-of-line regardless of terminal width.
    //
    // {elapsed_precise} is owned by indicatif so the seconds tick on
    // every steady_tick (every 120 ms below) without us re-calling
    // set_message.
    let width = Term::stdout()
        .size_checked()
        .map(|(_, w)| w as usize)
        .unwrap_or(80);
    let rule = "─".repeat(width);
    let template = format!(
        "\x1b[48;2;25;30;42m\x1b[38;2;90;95;105m{rule}\x1b[0m\n\
         \x1b[48;2;25;30;42m\x1b[38;2;180;200;225m \
         {{spinner}}  iter {{prefix}}  ·  {{elapsed_precise}}  ·  {{wide_msg}}\
         \x1b[0m"
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
