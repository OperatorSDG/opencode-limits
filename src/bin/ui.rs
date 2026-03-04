use chrono::{DateTime, Local};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use opencode_limits::types::UsageResponse;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style, Stylize},
    symbols::border,
    widgets::{Block, Borders, Gauge, Paragraph},
};
use std::io::stdout;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

struct AppState {
    rt: Runtime,
    client: reqwest::Client,
    usage: Option<UsageResponse>,
    last_refresh: Option<Instant>,
    last_error: Option<String>,
}

fn main() -> std::io::Result<()> {
    let mut state = AppState {
        rt: Runtime::new().expect("failed to initialize tokio runtime"),
        client: reqwest::Client::new(),
        usage: None,
        last_refresh: None,
        last_error: None,
    };
    let mut terminal = setup_terminal()?;

    refresh(&mut state);

    loop {
        maybe_auto_refresh(&mut state);

        // Render
        draw(&mut terminal, &state)?;

        // Quit
        match handle_input()? {
            UserAction::Quit => break,
            UserAction::Refresh => refresh(&mut state),
            UserAction::None => {}
        }
    }

    restore_terminal(&mut terminal)
}

fn setup_terminal() -> std::io::Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
    enable_raw_mode()?;

    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;

    Ok(terminal)
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> std::io::Result<()> {
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    disable_raw_mode()?;

    terminal.show_cursor()?;

    Ok(())
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &AppState,
) -> std::io::Result<()> {
    terminal.draw(|frame| render(frame, state))?;

    Ok(())
}

fn render(frame: &mut Frame, state: &AppState) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let status = build_status_line(state);
    let header = Paragraph::new(format!(
        "OpenCode Limits Monitor\nBackend: http://127.0.0.1:3000    {status}"
    ))
    .block(section_block("Header"));
    frame.render_widget(header, rows[0]);

    let profile =
        Paragraph::new(build_profile(state.usage.as_ref())).block(section_block("Profile"));
    frame.render_widget(profile, rows[1]);

    let (session_ratio, session_label) = remaining_meter(state.usage.as_ref(), true);
    render_split_gauge(
        frame,
        rows[2],
        "Session Left",
        session_ratio,
        &session_label,
    );

    let (weekly_ratio, weekly_label) = remaining_meter(state.usage.as_ref(), false);
    render_split_gauge(frame, rows[3], "Weekly Left", weekly_ratio, &weekly_label);

    let reset_text =
        Paragraph::new(build_resets(state.usage.as_ref())).block(section_block("Resets"));
    frame.render_widget(reset_text, rows[4]);

    let message = state
        .last_error
        .as_ref()
        .map(|e| format!("Last error: {e}"))
        .unwrap_or_else(|| "No errors".to_string());
    let message = Paragraph::new(message)
        .style(if state.last_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        })
        .block(section_block("Status"));
    frame.render_widget(message, rows[5]);

    let footer = Paragraph::new("q quit | r refresh now | auto-refresh every 5s")
        .block(section_block("Keys"));
    frame.render_widget(footer, rows[6]);
}

fn section_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(Color::Cyan))
}

fn render_split_gauge(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    title: &str,
    ratio: f64,
    label: &str,
) {
    let outer = section_block(title);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(3, 4), Constraint::Ratio(1, 4)])
        .split(inner);

    let gauge = Gauge::default()
        .gauge_style(remaining_style(ratio))
        .ratio(ratio)
        .label("");
    frame.render_widget(gauge, split[0]);

    let right = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(split[1]);

    let divider = Paragraph::new("│").style(Style::default().fg(Color::Cyan));
    frame.render_widget(divider, right[0]);

    let pct = Paragraph::new(label.to_string())
        .style(remaining_style(ratio))
        .alignment(Alignment::Center);
    frame.render_widget(pct, right[1]);
}

fn remaining_style(ratio: f64) -> Style {
    if ratio <= 0.1 {
        Style::default().fg(Color::Red).bold()
    } else if ratio <= 0.25 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    }
}

fn build_profile(usage: Option<&UsageResponse>) -> String {
    if let Some(usage) = usage {
        format!(
            "Email: {}\nPlan: {}",
            usage.data.email, usage.data.plan_type
        )
    } else {
        "Email: -\nPlan: -".to_string()
    }
}

fn build_resets(usage: Option<&UsageResponse>) -> String {
    if let Some(usage) = usage {
        format!(
            "Session reset: {}\nWeekly reset: {}",
            format_time_left(usage.data.session_reset_at),
            format_time_left(usage.data.weekly_reset_at)
        )
    } else {
        "Session reset: -\nWeekly reset: -".to_string()
    }
}

fn build_status_line(state: &AppState) -> String {
    if let Some(usage) = state.usage.as_ref() {
        format!("Last sync: {}", format_last_sync(&usage.last_sync_iso))
    } else {
        "Last sync: waiting for first successful refresh".to_string()
    }
}

fn format_last_sync(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.with_timezone(&Local).format("%-I:%M:%S %p").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

fn remaining_meter(usage: Option<&UsageResponse>, session: bool) -> (f64, String) {
    if let Some(usage) = usage {
        let used = if session {
            usage.data.session_used_percent
        } else {
            usage.data.weekly_used_percent
        };
        if let Some(used) = used {
            let used = used.min(100);
            let remaining = 100_u32.saturating_sub(used);
            return (f64::from(remaining) / 100.0, format!("{remaining}% left"));
        }
    }

    (0.0, "N/A".to_string())
}

fn format_time_left(reset_at: Option<i64>) -> String {
    let Some(reset_at) = reset_at else {
        return "unknown".to_string();
    };

    let now = chrono::Utc::now().timestamp();
    let mut secs = reset_at.saturating_sub(now);
    if secs <= 0 {
        return "now".to_string();
    }

    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let minutes = secs / 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

enum UserAction {
    None,
    Refresh,
    Quit,
}

fn handle_input() -> std::io::Result<UserAction> {
    if event::poll(Duration::from_millis(200))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                return Ok(match key.code {
                    KeyCode::Char('q') => UserAction::Quit,
                    KeyCode::Char('r') => UserAction::Refresh,
                    _ => UserAction::None,
                });
            }
        }
    }

    Ok(UserAction::None)
}

fn maybe_auto_refresh(state: &mut AppState) {
    let should_refresh = state
        .last_refresh
        .map(|t| t.elapsed() >= Duration::from_secs(5))
        .unwrap_or(true);

    if should_refresh {
        refresh(state);
    }
}

fn refresh(state: &mut AppState) {
    let result = state.rt.block_on(async {
        let resp = state
            .client
            .get("http://127.0.0.1:3000/usage")
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .and_then(|resp| resp.error_for_status())?;

        resp.json::<UsageResponse>().await
    });

    match result {
        Ok(usage) => {
            state.usage = Some(usage);
            state.last_error = None;
            state.last_refresh = Some(Instant::now());
        }
        Err(e) => {
            state.last_error = Some(e.to_string());
            state.last_refresh = Some(Instant::now());
        }
    }
}
