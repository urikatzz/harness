//! Ctrl-C cancellation for blocking calls (e.g. waiting on a model server).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

static PRESSED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    PRESSED.store(true, Ordering::SeqCst);
}

/// Runs `work` on a background thread and waits for it, returning `Err` if the
/// user presses Ctrl-C first. The SIGINT handler is only installed for the
/// duration of the call; the abandoned thread finishes (or times out) on its own.
pub fn cancellable<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    PRESSED.store(false, Ordering::SeqCst);
    let previous = unsafe { libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let outcome = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(value) => break Ok(value),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err("request thread panicked".to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) if PRESSED.load(Ordering::SeqCst) => {
                break Err("cancelled with ctrl+c".to_string())
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
    use std::sync::Mutex;

    // The SIGINT handler and PRESSED flag are process-wide, so these tests must not overlap.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn returns_work_result() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(cancellable(|| 7).unwrap(), 7);
    }

    #[test]
    fn ctrl_c_cancels_waiting() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let result = cancellable(|| {
            unsafe { libc::raise(libc::SIGINT) };
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        assert_eq!(result.unwrap_err(), "cancelled with ctrl+c");
    }
}
