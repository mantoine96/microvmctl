use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use super::logs::LogView;
use crate::metrics::{self, Rates, Sample};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Pause,
    Resume,
}

impl Action {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

/// Side effects the event loop performs on the app's behalf, keeping key
/// handling pure and testable.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Quit,
    Refresh,
    /// Run a microvmctl subcommand in the background and report its outcome.
    Run(Action, String),
    /// Hand the terminal to a microvmctl subcommand (console, ssh).
    Suspend(Vec<String>),
    Describe(String),
    Logs(String),
}

pub enum View {
    Table,
    Describe {
        vm: String,
        text: String,
        scroll: u16,
    },
    Logs(LogView),
}

pub enum Popup {
    Help,
    Confirm(Action, String),
}

#[derive(PartialEq)]
pub enum Mode {
    Normal,
    Filter,
}

pub struct Flash {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

const FLASH_TTL: Duration = Duration::from_secs(6);

pub struct App {
    pub hostname: String,
    pub is_root: bool,
    pub samples: Vec<Sample>,
    pub rates: HashMap<String, Rates>,
    pub table: TableState,
    pub filter: String,
    pub mode: Mode,
    pub view: View,
    pub popup: Option<Popup>,
    pub flash: Option<Flash>,
    /// Rows available to the scrolling views, set while drawing.
    pub viewport_height: usize,
}

impl App {
    pub fn new(hostname: String, is_root: bool) -> Self {
        Self {
            hostname,
            is_root,
            samples: Vec::new(),
            rates: HashMap::new(),
            table: TableState::default().with_selected(0),
            filter: String::new(),
            mode: Mode::Normal,
            view: View::Table,
            popup: None,
            flash: None,
            viewport_height: 20,
        }
    }

    pub fn update(&mut self, samples: Vec<Sample>) {
        let selected = self.selected().map(|s| s.name.clone());
        let previous: HashMap<&str, &Sample> =
            self.samples.iter().map(|s| (s.name.as_str(), s)).collect();
        self.rates = samples
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    metrics::rates(previous.get(s.name.as_str()).copied(), s),
                )
            })
            .collect();
        self.samples = samples;
        self.reselect(selected.as_deref());
    }

    pub fn visible(&self) -> Vec<&Sample> {
        let needle = self.filter.to_lowercase();
        self.samples
            .iter()
            .filter(|s| s.name.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn selected(&self) -> Option<&Sample> {
        self.visible().get(self.table.selected()?).copied()
    }

    fn reselect(&mut self, name: Option<&str>) {
        let visible = self.visible();
        let index = name
            .and_then(|n| visible.iter().position(|s| s.name == n))
            .or(self.table.selected())
            .map(|i| i.min(visible.len().saturating_sub(1)));
        self.table.select(if visible.is_empty() {
            None
        } else {
            index.or(Some(0))
        });
    }

    pub fn flash(&mut self, text: impl Into<String>, error: bool) {
        self.flash = Some(Flash {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    pub fn current_flash(&self) -> Option<&Flash> {
        self.flash.as_ref().filter(|f| f.at.elapsed() < FLASH_TTL)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        if let Some(popup) = self.popup.take() {
            return match (popup, key.code) {
                (Popup::Confirm(action, vm), KeyCode::Char('y' | 'Y')) => Effect::Run(action, vm),
                _ => Effect::None,
            };
        }
        if self.mode == Mode::Filter {
            self.filter_key(key);
            return Effect::None;
        }
        match &mut self.view {
            View::Table => self.table_key(key),
            View::Describe { scroll, .. } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.view = View::Table,
                    KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                    KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                    _ => {}
                }
                Effect::None
            }
            View::Logs(logs) => {
                let height = self.viewport_height;
                let page = height as isize;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.view = View::Table,
                    KeyCode::Char('j') | KeyCode::Down => logs.scroll(1, height),
                    KeyCode::Char('k') | KeyCode::Up => logs.scroll(-1, height),
                    KeyCode::PageDown | KeyCode::Char(' ') => logs.scroll(page, height),
                    KeyCode::PageUp | KeyCode::Char('b') => logs.scroll(-page, height),
                    KeyCode::Char('g') | KeyCode::Home => logs.top = Some(0),
                    KeyCode::Char('G') | KeyCode::End | KeyCode::Char('f') => logs.top = None,
                    _ => {}
                }
                Effect::None
            }
        }
    }

    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => {}
        }
        self.reselect(None);
    }

    fn table_key(&mut self, key: KeyEvent) -> Effect {
        let len = self.visible().len();
        let selected = self.selected().map(|s| (s.name.clone(), s.state.clone()));
        let on_selected = |f: &dyn Fn(String) -> Effect| match &selected {
            Some((name, _)) => f(name.clone()),
            None => Effect::None,
        };
        match key.code {
            KeyCode::Char('q') => Effect::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                self.table.select(Some(
                    (self.table.selected().unwrap_or(0) + 1).min(len.saturating_sub(1)),
                ));
                Effect::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.table
                    .select(Some(self.table.selected().unwrap_or(0).saturating_sub(1)));
                Effect::None
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.table.select(Some(0));
                Effect::None
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.table.select(Some(len.saturating_sub(1)));
                Effect::None
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                Effect::None
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.reselect(None);
                Effect::None
            }
            KeyCode::Char('?') => {
                self.popup = Some(Popup::Help);
                Effect::None
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Effect::Refresh,
            KeyCode::Enter | KeyCode::Char('d') => on_selected(&Effect::Describe),
            KeyCode::Char('l') => on_selected(&Effect::Logs),
            KeyCode::Char('a') => on_selected(&|vm| Effect::Suspend(vec!["console".into(), vm])),
            KeyCode::Char('s') => on_selected(&|vm| Effect::Suspend(vec!["ssh".into(), vm])),
            KeyCode::Char('u') => on_selected(&|vm| Effect::Run(Action::Start, vm)),
            KeyCode::Char('p') => match &selected {
                Some((vm, state)) if state == "paused" => Effect::Run(Action::Resume, vm.clone()),
                Some((vm, _)) => Effect::Run(Action::Pause, vm.clone()),
                None => Effect::None,
            },
            KeyCode::Char(c @ ('D' | 'R')) => {
                if let Some((vm, _)) = selected {
                    let action = if c == 'D' {
                        Action::Stop
                    } else {
                        Action::Restart
                    };
                    self.popup = Some(Popup::Confirm(action, vm));
                }
                Effect::None
            }
            _ => Effect::None,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample(name: &str, state: &str) -> Sample {
        Sample {
            at: Instant::now(),
            name: name.into(),
            state: state.into(),
            pid: Some(1),
            uptime: None,
            vcpus: Some(2),
            memory: Some(2 << 30),
            rss: None,
            cpu_time: None,
            cpu_usec: None,
            io: None,
            restarts: 0,
            address: None,
            outdated: false,
            api_denied: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn app() -> App {
        let mut app = App::new("hypervisor-host".into(), true);
        app.update(vec![
            sample("falcon", "running"),
            sample("osprey", "paused"),
        ]);
        app
    }

    #[test]
    fn navigates_and_acts_on_selection() {
        let mut app = app();
        assert_eq!(
            app.handle_key(key(KeyCode::Char('l'))),
            Effect::Logs("falcon".into())
        );
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected().unwrap().name, "osprey");
        assert_eq!(
            app.handle_key(key(KeyCode::Char('p'))),
            Effect::Run(Action::Resume, "osprey".into())
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('a'))),
            Effect::Suspend(vec!["console".into(), "osprey".into()])
        );
    }

    #[test]
    fn destructive_actions_need_confirmation() {
        let mut app = app();
        assert_eq!(app.handle_key(key(KeyCode::Char('D'))), Effect::None);
        assert_eq!(app.handle_key(key(KeyCode::Char('n'))), Effect::None);
        assert!(app.popup.is_none());

        app.handle_key(key(KeyCode::Char('R')));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Effect::Run(Action::Restart, "falcon".into())
        );
    }

    #[test]
    fn filter_narrows_rows_and_keeps_selection_valid() {
        let mut app = app();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "osp".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.visible().len(), 1);
        assert_eq!(app.selected().unwrap().name, "osprey");

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.visible().len(), 2);
    }

    #[test]
    fn selection_follows_vm_across_refreshes() {
        let mut app = app();
        app.handle_key(key(KeyCode::Char('j')));
        app.update(vec![
            sample("alpha", "running"),
            sample("falcon", "running"),
            sample("osprey", "paused"),
        ]);
        assert_eq!(app.selected().unwrap().name, "osprey");
    }
}
