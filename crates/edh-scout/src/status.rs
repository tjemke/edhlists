//! A one-line spinner on stderr for the wait before results show: what's being loaded or fetched
//! right now ("loading combo database", "EDHREC deck table for jodah-the-unifier") and how long
//! that step has taken so far. Lines meant to stay print above it.
//!
//! Without a terminal on stderr the spinner is hidden: steps say nothing, notes print as lines.

use std::io::IsTerminal;
use std::time::Duration;

use edh_data::http::Http;
use edh_data::progress::Progress;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// The browser header's spinner frames, so both look the same.
const FRAMES: [&str; 9] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", " "];

pub struct Status<'a> {
    pb: ProgressBar,
    visible: bool,
    /// Its notices (retries, back-offs) are held while the spinner runs and printed above it,
    /// since a stray line on stderr would break the spinner's redraw.
    http: Option<&'a Http>,
}

impl<'a> Status<'a> {
    pub fn new(http: Option<&'a Http>) -> Self {
        let visible = std::io::stderr().is_terminal();
        let pb = ProgressBar::new_spinner();
        if visible {
            // The last frame is the one shown when finished; it's cleared anyway.
            pb.set_style(
                ProgressStyle::with_template("  {spinner:.cyan} {msg} {elapsed:.dim}").unwrap().tick_strings(&FRAMES),
            );
            pb.enable_steady_tick(Duration::from_millis(120));
            if let Some(h) = http {
                h.set_quiet(true);
            }
        } else {
            pb.set_draw_target(ProgressDrawTarget::hidden());
        }
        Self { pb, visible, http: http.filter(|_| visible) }
    }

    /// Print held HTTP notices above the spinner.
    fn flush_notices(&self) {
        if let Some(h) = self.http {
            for n in h.take_notices() {
                self.pb.println(format!("  {n}"));
            }
        }
    }

    /// Run `f` with the spinner off the screen (for something else that draws: the picker).
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        self.pb.suspend(f)
    }
}

impl Progress for Status<'_> {
    fn step(&self, msg: &str) {
        self.flush_notices();
        self.pb.set_message(msg.to_string());
        self.pb.reset_elapsed();
    }

    fn note(&self, msg: &str) {
        self.flush_notices();
        if self.visible {
            self.pb.println(msg);
        } else {
            eprintln!("{msg}");
        }
    }
}

impl Drop for Status<'_> {
    fn drop(&mut self) {
        self.flush_notices();
        self.pb.finish_and_clear();
        if let Some(h) = self.http {
            h.set_quiet(false);
        }
    }
}
