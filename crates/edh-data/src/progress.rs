//! How slow steps say what they're doing. The CLI shows steps on a spinner; [`Lines`] prints them
//! as plain lines (the `sync` command, anything without a terminal to animate).

pub trait Progress {
    /// What's happening now, replacing the previous step: "loading the card database".
    fn step(&self, msg: &str);
    /// A line worth keeping on screen: results, warnings.
    fn note(&self, msg: &str);
}

/// Steps and notes as lines on stderr.
pub struct Lines;

impl Progress for Lines {
    fn step(&self, msg: &str) {
        eprintln!("{msg} ...");
    }

    fn note(&self, msg: &str) {
        eprintln!("{msg}");
    }
}

/// Says nothing (tests, callers that report on their own).
pub struct Silent;

impl Progress for Silent {
    fn step(&self, _: &str) {}
    fn note(&self, _: &str) {}
}
