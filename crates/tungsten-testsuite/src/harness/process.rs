//! Child process lifecycle for one test (SPEC §7.3): request/response with a watchdog, and
//! resume after a crash or timeout.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Extra time for the first case of a process (JVM start-up, later).
pub const STARTUP_ALLOWANCE: Duration = Duration::from_secs(5);

const STDERR_TAIL: usize = 2048;

/// What happened to one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// The body of an `Ok` response (without its tag).
    Returned(Vec<u8>),
    /// The driver caught a failure and reported it.
    Failed { kind: u8, message: String },
    /// The process died. `status` is its exit status.
    Crashed { status: String, stderr: String },
    /// The watchdog fired.
    TimedOut { stderr: String },
    /// The response didn't have the shape the protocol allows.
    Malformed(String),
}

enum Message {
    Ok(Vec<u8>),
    Failed(u8, String),
    Dead(String),
}

struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Message>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

fn spawn(bin: &Path, id: &str, body_len: usize) -> std::io::Result<Running> {
    let mut child = Command::new(bin)
        .arg(id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take();
    let mut stdout = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let sink = stderr.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = err.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut s = sink.lock().unwrap();
            s.extend_from_slice(&buf[..n]);
            let excess = s.len().saturating_sub(STDERR_TAIL);
            s.drain(..excess);
        }
    });
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let mut tag = [0u8; 1];
            if stdout.read_exact(&mut tag).is_err() {
                let _ = tx.send(Message::Dead("stdout closed".into()));
                return;
            }
            let msg = match tag[0] {
                0 => {
                    let mut body = vec![0u8; body_len];
                    match stdout.read_exact(&mut body) {
                        Ok(()) => Message::Ok(body),
                        Err(_) => Message::Dead("stdout closed inside a response".into()),
                    }
                }
                1 => {
                    let mut head = [0u8; 5];
                    if stdout.read_exact(&mut head).is_err() {
                        Message::Dead("stdout closed inside a failure".into())
                    } else {
                        let len = u32::from_le_bytes(head[1..5].try_into().unwrap()) as usize;
                        let mut text = vec![0u8; len.min(1 << 20)];
                        match stdout.read_exact(&mut text) {
                            Ok(()) => Message::Failed(head[0], String::from_utf8_lossy(&text).into_owned()),
                            Err(_) => Message::Dead("stdout closed inside a failure".into()),
                        }
                    }
                }
                other => Message::Dead(format!("unknown response tag {other:#04x}")),
            };
            let dead = matches!(msg, Message::Dead(_));
            if tx.send(msg).is_err() || dead {
                return;
            }
        }
    });
    Ok(Running { child, stdin, rx, stderr })
}

impl Running {
    fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sends each request in order and collects the outcomes. A crash or timeout scores that case,
/// kills the process and continues with the next case in a new one.
///
/// `body_len` is the size of an `Ok` response without its tag. `timeout` is per case.
pub fn run_cases(bin: &Path, id: &str, requests: &[Vec<u8>], body_len: usize, timeout: Duration) -> Vec<CaseOutcome> {
    run_cases_with(bin, id, requests, body_len, timeout, STARTUP_ALLOWANCE)
}

/// `run_cases` with an explicit start-up allowance for the first case of each process.
pub fn run_cases_with(
    bin: &Path,
    id: &str,
    requests: &[Vec<u8>],
    body_len: usize,
    timeout: Duration,
    startup: Duration,
) -> Vec<CaseOutcome> {
    let mut outcomes = Vec::with_capacity(requests.len());
    let mut proc: Option<Running> = None;
    let mut first_in_process = true;
    for req in requests {
        if proc.is_none() {
            match spawn(bin, id, body_len) {
                Ok(p) => {
                    proc = Some(p);
                    first_in_process = true;
                }
                Err(e) => {
                    outcomes.push(CaseOutcome::Crashed {
                        status: format!("could not start {}: {e}", bin.display()),
                        stderr: String::new(),
                    });
                    continue;
                }
            }
        }
        let p = proc.as_mut().unwrap();
        let limit = if first_in_process { timeout + startup } else { timeout };
        first_in_process = false;
        let wrote = p
            .stdin
            .as_mut()
            .map(|s| s.write_all(req).and_then(|_| s.flush()))
            .unwrap_or(Err(std::io::ErrorKind::BrokenPipe.into()));
        // A failed write is reported through the reader (the process is dead), so wait for it.
        let msg = match p.rx.recv_timeout(limit) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Message::Dead("reader stopped".into())),
        };
        match msg {
            Some(Message::Ok(body)) => outcomes.push(CaseOutcome::Returned(body)),
            Some(Message::Failed(kind, message)) => outcomes.push(CaseOutcome::Failed { kind, message }),
            Some(Message::Dead(why)) => {
                let mut p = proc.take().unwrap();
                let malformed = why.starts_with("unknown response tag") || why.contains("inside a");
                let status = if malformed {
                    p.kill();
                    String::new()
                } else {
                    match p.child.wait() {
                        Ok(s) => format!("{s} ({why}{})", if wrote.is_err() { ", write failed" } else { "" }),
                        Err(e) => format!("unknown ({e})"),
                    }
                };
                // Give the stderr thread a moment to see the end of the stream.
                thread::sleep(Duration::from_millis(20));
                let stderr = p.stderr_tail();
                outcomes.push(if malformed {
                    CaseOutcome::Malformed(format!("{why}; stderr: {stderr}"))
                } else {
                    CaseOutcome::Crashed { status, stderr }
                });
            }
            None => {
                let mut p = proc.take().unwrap();
                p.kill();
                thread::sleep(Duration::from_millis(20));
                outcomes.push(CaseOutcome::TimedOut { stderr: p.stderr_tail() });
            }
        }
    }
    if let Some(mut p) = proc {
        drop(p.stdin.take());
        let _ = p.child.wait();
    }
    outcomes
}
