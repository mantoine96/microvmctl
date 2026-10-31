use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, Wrap};

use super::app::{App, Mode, Popup, View};
use crate::fmt::{self, opt};

const ACCENT: Color = Color::Cyan;

const TABLE_KEYS: &[(&str, &str)] = &[
    ("enter/d", "describe"),
    ("l", "logs"),
    ("a", "attach console"),
    ("s", "ssh"),
    ("u", "start"),
    ("D", "stop"),
    ("R", "restart"),
    ("p", "pause/resume"),
    ("/", "filter"),
    ("?", "help"),
    ("q", "quit"),
];

const LOG_KEYS: &[(&str, &str)] = &[
    ("j/k", "scroll"),
    ("space/b", "page"),
    ("g", "top"),
    ("G/f", "follow"),
    ("esc", "back"),
];

const DESCRIBE_KEYS: &[(&str, &str)] = &[("j/k", "scroll"), ("esc", "back")];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);
    match &app.view {
        View::Table => draw_table(frame, app, body),
        View::Describe { .. } => draw_describe(frame, app, body),
        View::Logs(_) => draw_logs(frame, app, body),
    }
    draw_footer(frame, app, footer);

    match &app.popup {
        Some(Popup::Help) => draw_help(frame),
        Some(Popup::Confirm(action, vm)) => draw_confirm(frame, action.verb(), vm),
        None => {}
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let privilege = if app.is_root {
        Span::styled("root", Style::new().fg(Color::Green))
    } else {
        Span::styled(
            "read-only (run with sudo for actions and I/O rates)",
            Style::new().fg(Color::Yellow),
        )
    };
    let info = Line::from(vec![
        " microvmctl ".bold().fg(Color::Black).bg(ACCENT),
        format!("  host: {}  vms: {}  ", app.hostname, app.samples.len()).into(),
        privilege,
    ]);
    let keys = match app.view {
        View::Table => TABLE_KEYS,
        View::Describe { .. } => DESCRIBE_KEYS,
        View::Logs(_) => LOG_KEYS,
    };
    let hints = Line::from(
        keys.iter()
            .flat_map(|(k, v)| {
                [
                    Span::styled(format!(" <{k}>"), Style::new().fg(ACCENT).bold()),
                    Span::raw(format!(" {v}")),
                ]
            })
            .collect::<Vec<_>>(),
    )
    .dark_gray();
    frame.render_widget(Paragraph::new(vec![info, hints]), area);
}

fn state_style(state: &str) -> Style {
    let color = match state {
        "running" => Color::Green,
        "paused" => Color::Yellow,
        "failed" => Color::Red,
        "activating" | "deactivating" | "reloading" => Color::Cyan,
        _ => Color::DarkGray,
    };
    Style::new().fg(color)
}

fn block(title: String) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(title).bold().centered())
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    const HEADER: [&str; 14] = [
        "NAME", "STATE", "PID", "UPTIME", "VCPU", "MEM", "RSS", "CPU%", "DISK R/s", "DISK W/s",
        "NET RX/s", "NET TX/s", "ADDRESS", "NOTES",
    ];
    let visible = app.visible();
    let cells: Vec<[String; 14]> = visible
        .iter()
        .map(|s| {
            let r = app.rates.get(&s.name).copied().unwrap_or_default();
            [
                s.name.clone(),
                s.state.clone(),
                opt(s.pid, |p| p.to_string()),
                opt(s.uptime, fmt::duration),
                opt(s.vcpus, |v| v.to_string()),
                opt(s.memory, fmt::bytes),
                opt(s.rss, fmt::bytes),
                opt(r.cpu, |c| format!("{c:.1}")),
                opt(r.disk_read, fmt::rate),
                opt(r.disk_write, fmt::rate),
                opt(r.net_rx, fmt::rate),
                opt(r.net_tx, fmt::rate),
                opt(s.address.clone(), |a| a),
                s.notes(),
            ]
        })
        .collect();

    // Size columns to their content like k9s; the last one takes the slack.
    let widths: Vec<Constraint> = HEADER
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let width = cells
                .iter()
                .map(|row| row[i].chars().count())
                .chain([h.len()])
                .max()
                .unwrap_or(0) as u16;
            if i == HEADER.len() - 1 {
                Constraint::Fill(1)
            } else {
                Constraint::Length(width)
            }
        })
        .collect();

    let rows: Vec<Row> = visible
        .iter()
        .zip(cells)
        .map(|(s, cells)| {
            Row::new(cells.into_iter().enumerate().map(|(i, c)| {
                let cell = Cell::from(c);
                match i {
                    1 => cell.style(state_style(&s.state)),
                    13 => cell.style(Style::new().fg(Color::Yellow)),
                    _ => cell,
                }
            }))
        })
        .collect();
    let count = visible.len();
    let scope = if app.filter.is_empty() {
        "all".to_string()
    } else {
        format!("/{}", app.filter)
    };
    let table = Table::new(rows, widths)
        .header(Row::new(HEADER).style(Style::new().bold().fg(Color::White)))
        .block(block(format!(" VMs({scope})[{count}] ")))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .column_spacing(3);
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn draw_describe(frame: &mut Frame, app: &App, area: Rect) {
    let View::Describe { vm, text, scroll } = &app.view else {
        return;
    };
    let paragraph = Paragraph::new(text.as_str())
        .block(block(format!(" describe({vm}) ")))
        .scroll((*scroll, 0));
    frame.render_widget(paragraph, area);
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    app.viewport_height = area.height.saturating_sub(2) as usize;
    let View::Logs(logs) = &app.view else {
        return;
    };
    let mode = if logs.following() {
        "following"
    } else {
        "paused"
    };
    let lines: Vec<Line> = logs
        .window(app.viewport_height)
        .into_iter()
        .map(Line::from)
        .collect();
    let paragraph =
        Paragraph::new(Text::from(lines)).block(block(format!(" logs({}) [{mode}] ", logs.vm)));
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = if app.mode == Mode::Filter {
        Line::from(vec![
            Span::styled(" /", Style::new().fg(ACCENT).bold()),
            Span::raw(app.filter.as_str()),
            Span::styled("█", Style::new().fg(ACCENT)),
        ])
    } else if let Some(flash) = app.current_flash() {
        let color = if flash.error {
            Color::Red
        } else {
            Color::Green
        };
        Line::from(Span::styled(
            format!(" {}", flash.text),
            Style::new().fg(color),
        ))
    } else {
        Line::default()
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn draw_confirm(frame: &mut Frame, verb: &str, vm: &str) {
    let area = centered(frame.area(), 44, 5);
    let text = Text::from(vec![
        Line::from(format!("{verb} {vm}?")).bold().centered(),
        Line::from(vec![
            Span::styled("y", Style::new().fg(ACCENT).bold()),
            " confirm   ".into(),
            Span::styled("any other key", Style::new().fg(ACCENT).bold()),
            " cancel".into(),
        ])
        .centered(),
    ]);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(block(" confirm ".into()).border_style(Style::new().fg(Color::Red))),
        area,
    );
}

fn draw_help(frame: &mut Frame) {
    let section = |title: &str, keys: &[(&str, &str)]| {
        let mut lines = vec![Line::from(title.to_string()).bold().fg(ACCENT)];
        lines.extend(keys.iter().map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("  {k:<10}"), Style::new().bold()),
                Span::raw(v.to_string()),
            ])
        }));
        lines.push(Line::default());
        lines
    };
    let mut lines = section("VMs", TABLE_KEYS);
    lines.extend(section("Logs", LOG_KEYS));
    lines.push(Line::from(
        "Console and ssh hand over the terminal; exit them to come back.",
    ));
    lines.push(Line::from(
        "Ctrl-r refreshes now, Ctrl-c quits from anywhere.",
    ));

    let area = centered(frame.area(), 70, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(" help ".into())),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::tests::sample;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 12)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_vm_table_and_confirmation() {
        let mut app = App::new("hypervisor-host".into(), false);
        app.update(vec![
            sample("falcon", "running"),
            sample("osprey", "paused"),
        ]);

        let screen = render(&mut app);
        assert!(screen.contains("host: hypervisor-host"), "{screen}");
        assert!(screen.contains("read-only"), "{screen}");
        assert!(screen.contains("VMs(all)[2]"), "{screen}");
        assert!(
            screen.contains("osprey") && screen.contains("paused"),
            "{screen}"
        );

        app.popup = Some(Popup::Confirm(
            crate::tui::app::Action::Stop,
            "osprey".into(),
        ));
        assert!(render(&mut app).contains("stop osprey?"));
    }
}
