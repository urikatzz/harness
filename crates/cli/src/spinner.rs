//! "⠋ thinking… 3s" on one line while waiting for the model; erased when dropped.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Spinner {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Starts drawing on stderr; does nothing when stderr isn't a terminal.
    pub fn start(label: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if !std::io::stderr().is_terminal() {
            return Self { stop, thread: None };
        }
        let (flag, label) = (stop.clone(), label.to_string());
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut err = std::io::stderr();
            for frame in FRAMES.iter().cycle() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let secs = started.elapsed().as_secs();
                let _ = write!(err, "\r\x1b[2K\x1b[2m{frame} {label}… {secs}s  (ctrl+c to cancel)\x1b[0m");
                let _ = err.flush();
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        });
        Self { stop, thread: Some(thread) }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
