//! Ctrl-C cancellation for blocking calls (e.g. waiting on a model server).

use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

// A signal handler can only reach global state. The lock makes overlapping calls take turns,
// so one call can't reset the flag or the handler while another is waiting.
static PRESSED: AtomicBool = AtomicBool::new(false);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

extern "C" fn on_sigint(_: libc::c_int) {
    PRESSED.store(true, Ordering::SeqCst);
}

/// Runs `work` on a background thread and waits for it, returning `Err` if the
/// user presses Ctrl-C first. The SIGINT handler is only installed for the
/// duration of the call; the abandoned thread finishes (or times out) on its own.
pub fn cancellable<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    PRESSED.store(false, Ordering::SeqCst);
    let previous = unsafe { libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let outcome = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(value) => break Ok(value),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err(Error::from("request thread panicked")),
            Err(mpsc::RecvTimeoutError::Timeout) if PRESSED.load(Ordering::SeqCst) => {
                break Err(Error::Cancelled)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    unsafe { libc::signal(libc::SIGINT, previous) };
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_work_result() {
        assert_eq!(cancellable(|| 7).unwrap(), 7);
    }

    #[test]
    fn ctrl_c_cancels_waiting() {
        let result = cancellable(|| {
            unsafe { libc::raise(libc::SIGINT) };
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
