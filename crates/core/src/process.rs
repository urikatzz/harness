//! Bounded subprocesses, including their descendants.
use crate::error::Result;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const INPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn capture(mut reader: impl Read, exceeded: Arc<AtomicBool>) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            return Ok(output);
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..n.min(remaining)]);
        if n > remaining {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(unix)]
fn terminate_group(id: u32) {
    // The child starts a fresh process group; terminate inherited pipes and grandchildren too.
    unsafe {
        libc::kill(-(id as i32), libc::SIGKILL);
    }
}

pub fn run(mut command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output> {
    if input.len() > INPUT_LIMIT {
        return Err("subprocess input exceeds 8 MiB".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let out_flag = exceeded.clone();
    let err_flag = exceeded.clone();
    let out = std::thread::spawn(move || capture(stdout, out_flag));
    let err = std::thread::spawn(move || capture(stderr, err_flag));
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let started = Instant::now();
    let mut status = None;
    let failure = loop {
        if exceeded.load(Ordering::Relaxed) {
            break Some("subprocess output exceeds 1 MiB");
        }
        if started.elapsed() >= timeout {
            break Some("subprocess timed out");
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(result) => status = result,
                Err(_) => break Some("could not wait for subprocess"),
            }
        }
        if status.is_some() && out.is_finished() && err.is_finished() && writer.is_finished() {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    #[cfg(unix)]
    terminate_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
    let stdout = out.join().map_err(|_| "stdout reader failed")??;
    let stderr = err.join().map_err(|_| "stderr reader failed")??;
    let write_result = writer.join().map_err(|_| "stdin writer failed")?;
    if let Some(reason) = failure {
        return Err(reason.into());
    }
    if exceeded.load(Ordering::Relaxed) {
        return Err("subprocess output exceeds 1 MiB".into());
    }
    // Broken pipe is common when a command does not consume its arguments on stdin.
    if let Err(e) = write_result {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e.into());
        }
    }
    Ok(Output {
        status: status.unwrap(),
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn bounds_runtime_output_and_inherited_pipes() {
        assert!(run(shell("sleep 5"), vec![], Duration::from_millis(100))
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert!(run(
            shell("sleep 5 & exit 0"),
            vec![],
            Duration::from_millis(100)
        )
        .unwrap_err()
        .to_string()
        .contains("timed out"));
        assert!(run(shell("yes x"), vec![], Duration::from_secs(5))
            .unwrap_err()
            .to_string()
            .contains("output exceeds"));
        let result = run(
            shell("cat; printf warning >&2"),
            b"hello".to_vec(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"warning");
    }

}
