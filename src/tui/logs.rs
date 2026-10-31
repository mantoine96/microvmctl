use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};

const MAX_LINES: usize = 10_000;

/// A `journalctl --follow` child whose output accumulates in a bounded buffer.
pub struct LogView {
    pub vm: String,
    lines: Arc<Mutex<VecDeque<String>>>,
    child: Child,
    /// First visible line, or `None` to stick to the tail.
    pub top: Option<usize>,
}

impl LogView {
    pub fn follow(vm: &str, unit: &str) -> Result<Self> {
        let mut child = Command::new("journalctl")
            .args(["--all", "--follow", "--lines=1000", "--no-pager"])
            .arg(format!("--unit={unit}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting journalctl")?;
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = Arc::clone(&lines);
            thread::spawn(move || {
                for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                    let mut buf = lines.lock().unwrap();
                    if buf.len() == MAX_LINES {
                        buf.pop_front();
                    }
                    buf.push_back(sanitize(&String::from_utf8_lossy(&line)));
                }
            });
        }
        Ok(Self {
            vm: vm.into(),
            lines,
            child,
            top: None,
        })
    }

    pub fn len(&self) -> usize {
        self.lines.lock().unwrap().len()
    }

    /// The lines to show in a viewport of `height` rows.
    pub fn window(&self, height: usize) -> Vec<String> {
        let lines = self.lines.lock().unwrap();
        let tail = lines.len().saturating_sub(height);
        let start = self.top.map_or(tail, |t| t.min(tail));
        lines.iter().skip(start).take(height).cloned().collect()
    }

    /// Scrolls by `delta` lines; reaching the bottom resumes following.
    pub fn scroll(&mut self, delta: isize, height: usize) {
        let tail = self.len().saturating_sub(height);
        let top = self
            .top
            .unwrap_or(tail)
            .saturating_add_signed(delta)
            .min(tail);
        self.top = (top < tail).then_some(top);
    }

    pub fn following(&self) -> bool {
        self.top.is_none()
    }
}

impl Drop for LogView {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Drops ANSI escape sequences and control characters: the guest console
/// colors its output, and raw escapes would corrupt the TUI.
pub fn sanitize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then a final byte in @..~
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: terminated by BEL or ESC \
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_console_escapes() {
        assert_eq!(
            sanitize("\x1b[1;32m<<< Welcome to NixOS >>>\x1b[0m\r"),
            "<<< Welcome to NixOS >>>"
        );
        assert_eq!(
            sanitize("\x1b]3008;end=7eb2\x1b\\[  186.2] reboot: Power down"),
            "[  186.2] reboot: Power down"
        );
        assert_eq!(sanitize("a\tb\x07"), "a    b");
    }
}
