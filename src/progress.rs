//! Progress display for dependency updates
//!
//! Provides visual feedback during the update workflow using indicatif.

use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Progress reporter for the update workflow
pub struct Progress {
    /// Whether progress display is enabled (disabled in quiet mode)
    enabled: bool,
    /// Current progress bar
    bar: Option<ProgressBar>,
}

impl Progress {
    /// Create a new progress reporter
    pub fn new(enabled: bool) -> Self {
        Self { enabled, bar: None }
    }

    /// Create a disabled progress reporter
    pub fn disabled() -> Self {
        Self::new(false)
    }

    /// Show a spinner with a message for an indeterminate operation
    pub fn spinner(&mut self, message: &str) {
        if !self.enabled {
            return;
        }

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner:.cyan} {msg}")
                .expect("Invalid template"),
        );
        spinner.set_message(message.to_string());
        spinner.enable_steady_tick(Duration::from_millis(80));
        self.bar = Some(spinner);
    }

    /// Start a progress bar for a known number of items
    pub fn start(&mut self, total: u64, message: &str) {
        if !self.enabled {
            return;
        }

        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.cyan} {msg} [{bar:30.cyan/blue}] {pos}/{len} ({eta})")
                .expect("Invalid template")
                .progress_chars("█▓▒░"),
        );
        bar.set_message(message.to_string());
        bar.enable_steady_tick(Duration::from_millis(100));
        self.bar = Some(bar);
    }

    /// Increment progress by one
    pub fn inc(&self) {
        if let Some(ref bar) = self.bar {
            bar.inc(1);
        }
    }

    /// Update the message
    pub fn set_message(&self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.set_message(message.to_string());
        }
    }

    /// Finish the current progress bar with a message
    pub fn finish(&mut self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.finish_with_message(message.to_string());
        }
        self.bar = None;
    }

    /// Finish and clear the current progress bar
    pub fn finish_and_clear(&mut self) {
        if let Some(ref bar) = self.bar {
            bar.finish_and_clear();
        }
        self.bar = None;
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_disabled() {
        let mut progress = Progress::disabled();
        progress.spinner("test");
        progress.start(10, "test");
        progress.inc();
        progress.set_message("test");
        progress.finish("done");
    }

    #[test]
    fn test_progress_enabled() {
        let mut progress = Progress::new(true);
        progress.start(3, "Processing");
        progress.inc();
        progress.set_message("item 1");
        progress.inc();
        progress.finish_and_clear();
    }
}
