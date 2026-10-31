//! k9s-style interactive view. Every action shells out to a microvmctl
//! subcommand, so the TUI behaves (and fails) exactly like the CLI.

mod app;
mod logs;
mod ui;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use self::app::{Action, App, Effect, View};
use self::logs::LogView;
use crate::cmd::status;
use crate::metrics::{self, Sample};
use crate::vm;

const REFRESH: Duration = Duration::from_secs(2);
const INPUT_POLL: Duration = Duration::from_millis(100);

enum Message {
    Samples(Result<Vec<Sample>>),
    Finished(Action, String, Result<String, String>),
}

pub fn run(state_dir: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("locating the microvmctl binary")?;
    let hostname = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    let mut app = App::new(hostname, rustix::process::geteuid().is_root());

    let (tx, rx) = mpsc::channel();
    let wake = spawn_refresher(state_dir.to_path_buf(), tx.clone());

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, state_dir, &exe, &rx, &tx, &wake);
    ratatui::restore();
    result
}

/// Samples in the background so slow systemctl/API calls never stall input.
/// Sending on the returned channel triggers an immediate refresh.
fn spawn_refresher(state_dir: PathBuf, tx: Sender<Message>) -> Sender<()> {
    let (wake_tx, wake_rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if tx
                .send(Message::Samples(metrics::collect(&state_dir)))
                .is_err()
            {
                return;
            }
            match wake_rx.recv_timeout(REFRESH) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    wake_tx
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    state_dir: &Path,
    exe: &Path,
    rx: &Receiver<Message>,
    tx: &Sender<Message>,
    wake: &Sender<()>,
) -> Result<()> {
    loop {
        while let Ok(message) = rx.try_recv() {
            match message {
                Message::Samples(Ok(samples)) => app.update(samples),
                Message::Samples(Err(e)) => app.flash(format!("refresh failed: {e:#}"), true),
                Message::Finished(action, vm, outcome) => {
                    match outcome {
                        Ok(note) if note.is_empty() => {
                            app.flash(format!("{} {vm}: done", action.verb()), false)
                        }
                        Ok(note) => app.flash(format!("{} {vm}: {note}", action.verb()), false),
                        Err(e) => app.flash(format!("{} {vm} failed: {e}", action.verb()), true),
                    }
                    let _ = wake.send(());
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if !event::poll(INPUT_POLL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Refresh => {
                let _ = wake.send(());
            }
            Effect::Run(action, vm) => {
                app.flash(format!("{} {vm}...", action.verb()), false);
                spawn_action(exe.to_path_buf(), action, vm, tx.clone());
            }
            Effect::Suspend(args) => {
                ratatui::restore();
                let outcome = run_in_foreground(exe, &args);
                *terminal = ratatui::init();
                terminal.clear()?;
                if let Err(e) = outcome {
                    app.flash(format!("{}: {e:#}", args.join(" ")), true);
                }
                let _ = wake.send(());
            }
            Effect::Describe(name) => {
                match vm::find(state_dir, &name).and_then(|vm| status::report(&vm, false)) {
                    Ok(text) => {
                        app.view = View::Describe {
                            vm: name,
                            text,
                            scroll: 0,
                        }
                    }
                    Err(e) => app.flash(format!("describe {name}: {e:#}"), true),
                }
            }
            Effect::Logs(name) => {
                match vm::find(state_dir, &name).and_then(|vm| LogView::follow(&name, &vm.unit())) {
                    Ok(logs) => app.view = View::Logs(logs),
                    Err(e) => app.flash(format!("logs {name}: {e:#}"), true),
                }
            }
        }
    }
}

fn spawn_action(exe: PathBuf, action: Action, vm: String, tx: Sender<Message>) {
    thread::spawn(move || {
        let outcome = Command::new(&exe)
            .args([action.verb(), &vm])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())
            .and_then(|out| {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let stderr = stderr
                    .strip_prefix("Error: ")
                    .unwrap_or(&stderr)
                    .to_string();
                // Keep the first line: multi-line hints don't fit the footer.
                let first = stderr.lines().next().unwrap_or_default().to_string();
                if out.status.success() {
                    Ok(first)
                } else {
                    Err(first)
                }
            });
        let _ = tx.send(Message::Finished(action, vm, outcome));
    });
}

/// Runs e.g. `microvmctl console <vm>` on the real terminal. On failure,
/// keeps its error visible (such as the hint to enable `--console pty`)
/// until the user is ready to return.
fn run_in_foreground(exe: &Path, args: &[String]) -> Result<()> {
    let status = Command::new(exe).args(args).status()?;
    if !status.success() {
        print!("\n[press Enter to return to microvmctl]");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
    }
    Ok(())
}
