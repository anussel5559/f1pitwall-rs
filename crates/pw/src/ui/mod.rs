pub mod board;
pub mod login;
pub mod picker;
pub mod race_control;
pub mod style;
pub mod telemetry;
pub mod track_map;
pub mod weather;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::AppState;
use f1core::db::DriverLocation;
use f1core::telemetry::SharedTelemetry;

pub fn draw(
    f: &mut Frame,
    state: &mut AppState,
    telem: Option<&SharedTelemetry>,
    locations: &[DriverLocation],
) {
    use crate::app::ViewMode;

    match &state.view_mode {
        ViewMode::Telemetry { .. } => {
            telemetry::render_telemetry(f, f.area(), state, telem);
        }
        ViewMode::TrackMap => {
            track_map::render_track_map(f, f.area(), state, locations);
        }
        ViewMode::Board => {
            draw_race(f, state);
        }
    }

    // Render toasts as an overlay
    render_toasts(f, state);
}

fn draw_race(f: &mut Frame, state: &mut AppState) {
    if state.show_race_control {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // timing board
                Constraint::Length(8), // race control
                Constraint::Length(3), // weather
                Constraint::Length(1), // help
            ])
            .split(f.area());

        board::render_board(f, chunks[0], state);
        race_control::render_race_control(f, chunks[1], state);
        weather::render_weather(f, chunks[2], state);
        render_board_help(f, chunks[3], state);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // timing board
                Constraint::Length(3), // weather
                Constraint::Length(1), // help
            ])
            .split(f.area());

        board::render_board(f, chunks[0], state);
        weather::render_weather(f, chunks[1], state);
        render_board_help(f, chunks[2], state);
    }
}

fn render_board_help(f: &mut Frame, area: Rect, state: &AppState) {
    let key = Style::default().fg(Color::Yellow);
    let dim = Style::default().fg(Color::DarkGray);
    let map_enabled = state.authenticated || !state.clock.is_live;
    let map_style = if map_enabled { key } else { dim };
    let map_label_style = if map_enabled {
        dim
    } else {
        Style::default().fg(Color::Indexed(238))
    };

    let mut spans = vec![
        Span::styled(" ↑↓", key),
        Span::styled(" nav  ", dim),
        Span::styled("t", key),
        Span::styled(" telem  ", dim),
        Span::styled("m", map_style),
        Span::styled(" map  ", map_label_style),
        Span::styled("Space", map_style),
        Span::styled(" pin  ", map_label_style),
        Span::styled("r", key),
        Span::styled(" race ctrl  ", dim),
        Span::styled("←→", key),
        Span::styled(" seek  ", dim),
        Span::styled("⇧←→", key),
        Span::styled(" 60s  ", dim),
        Span::styled("p", key),
        Span::styled(" pause  ", dim),
        Span::styled("q", key),
        Span::styled(" quit", dim),
    ];

    if !map_enabled {
        spans.push(Span::styled("   (login for live map)", dim));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_toasts(f: &mut Frame, state: &AppState) {
    let toasts = state.active_toasts();
    if toasts.is_empty() {
        return;
    }
    let area = f.area();
    let inner_width = area.width.saturating_sub(2) as usize;
    let wrapped_lines: usize = toasts
        .iter()
        .map(|(m, _)| (m.len() / inner_width.max(1)) + 1)
        .sum();
    let toast_height = (wrapped_lines as u16 + 2).min(area.height.saturating_sub(2));

    let toast_area = Rect {
        x: 0,
        y: area.height.saturating_sub(toast_height),
        width: area.width,
        height: toast_height,
    };

    let lines: Vec<Line> = toasts
        .iter()
        .map(|(msg, is_error)| {
            let color = if *is_error { Color::Red } else { Color::Green };
            Line::from(Span::styled(msg.as_str(), Style::default().fg(color)))
        })
        .collect();

    let toast_block = Paragraph::new(lines)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Status "),
        );

    f.render_widget(Clear, toast_area);
    f.render_widget(toast_block, toast_area);
}
