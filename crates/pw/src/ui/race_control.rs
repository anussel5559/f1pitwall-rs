use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use crate::app::AppState;

fn flag_color(flag: &str) -> Color {
    match flag.to_uppercase().as_str() {
        "RED" => Color::Red,
        "YELLOW" | "DOUBLE YELLOW" => Color::Yellow,
        "GREEN" => Color::Green,
        "BLUE" => Color::Blue,
        "BLACK AND WHITE" => Color::White,
        _ => Color::Gray,
    }
}

/// Format a race control message timestamp as local time (HH:MM:SS) or UTC fallback.
fn format_rc_time(date: &str, state: &AppState) -> String {
    if let Ok(dt) = date.parse::<DateTime<Utc>>() {
        if let Some(offset) = state.clock.local_offset {
            return dt.with_timezone(&offset).format("%H:%M:%S").to_string();
        }
        return dt.format("%H:%M:%S").to_string();
    }
    String::new()
}

pub fn render_race_control(f: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .session
        .race_control
        .iter()
        // newest first — the query returns DESC order, so latest messages
        // are always visible at the top even when the panel is small.
        .map(|msg| {
            let time_str = format_rc_time(&msg.date, state);
            let lap_str = msg
                .lap_number
                .map(|l| format!("L{:>2}", l))
                .unwrap_or_else(|| "   ".to_string());

            let color = flag_color(&msg.flag);

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} {} ", time_str, lap_str),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(&msg.message, Style::default().fg(color)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Race Control ")
            .border_style(Style::default().fg(Color::Yellow)),
    );

    f.render_widget(list, area);
}
