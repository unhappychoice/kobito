use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub fn make_status_bar() -> ProgressBar {
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    bar.enable_steady_tick(Duration::from_millis(120));
    bar
}

pub fn set_status(
    bar: &ProgressBar,
    iteration: u32,
    elapsed: Duration,
    retries: u32,
    state: &str,
) {
    let secs = elapsed.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    bar.set_message(format!(
        "iter {iteration} · {h:02}:{m:02}:{s:02} · retry {retries} · {state}"
    ));
}
